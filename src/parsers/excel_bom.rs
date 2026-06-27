use std::path::Path;
use std::collections::HashMap;
use anyhow::{Result, Context, anyhow};
use calamine::{Reader, open_workbook_auto, Data, DataType};

use crate::models::Component;

/// Parsea una lista de materiales (BOM) en formato Excel (.xlsx, .xls, .ods).
/// Detecta dinámicamente las columnas principales y procesa designadores agrupados.
pub fn parse_excel_bom<P: AsRef<Path>>(path: P) -> Result<HashMap<String, Component>> {
    let mut workbook = open_workbook_auto(&path)
        .context("No se pudo abrir el archivo Excel de BOM")?;
        
    let sheet_name = workbook.sheet_names()
        .first()
        .cloned()
        .ok_or_else(|| anyhow!("El libro de Excel no contiene hojas de trabajo"))?;
        
    let range = workbook.worksheet_range(&sheet_name)
        .context(format!("No se pudo leer la hoja: {}", sheet_name))?;
        
    let mut components = HashMap::new();
    
    let mut header_map = HashMap::new();
    let mut header_row_idx = None;
    
    for (row_idx, row) in range.rows().enumerate() {
        if row_idx > 10 { break; }
        
        for (col_idx, cell) in row.iter().enumerate() {
            if let Some(text) = cell.get_string() {
                let lower = text.trim().to_lowercase();
                if lower == "designator" || lower == "refdes" || lower == "referencia" {
                    header_map.insert("designator", col_idx);
                } else if lower == "mpn" || lower == "part number" || lower == "código" {
                    header_map.insert("mpn", col_idx);
                } else if lower == "manufacturer" || lower == "fabricante" || lower == "brand" {
                    header_map.insert("manufacturer", col_idx);
                } else if lower == "value" || lower == "valor" {
                    header_map.insert("value", col_idx);
                } else if lower == "footprint" || lower == "package" || lower == "encapsulado" {
                    header_map.insert("footprint", col_idx);
                }
            }
        }
        
        if header_map.contains_key("designator") {
            header_row_idx = Some(row_idx);
            break;
        }
    }
    
    let designator_col = header_map.get("designator")
        .copied()
        .ok_or_else(|| anyhow!("No se encontró la columna de designadores ('Designator') en el archivo Excel"))?;
        
    let start_row = header_row_idx.unwrap_or(0) + 1;
    
    for row_idx in start_row..range.height() {
        let row = &range[row_idx];
        
        let des_cell = &row[designator_col];
        let des_str = match des_cell {
            Data::String(s) => s.trim().to_string(),
            Data::Int(i) => i.to_string(),
            Data::Float(f) => f.to_string(),
            _ => continue,
        };
        
        if des_str.is_empty() { continue; }
        
        let mpn = header_map.get("mpn").and_then(|&c| row.get(c)).and_then(|cell| get_cell_as_string(cell));
        let manufacturer = header_map.get("manufacturer").and_then(|&c| row.get(c)).and_then(|cell| get_cell_as_string(cell));
        let value = header_map.get("value").and_then(|&c| row.get(c)).and_then(|cell| get_cell_as_string(cell));
        let footprint = header_map.get("footprint").and_then(|&c| row.get(c)).and_then(|cell| get_cell_as_string(cell)).unwrap_or_default();
        
        let parsed_designators = expand_designators(&des_str);
        
        for des in parsed_designators {
            let mut comp = Component::new(&des, &footprint);
            if let Some(ref val) = value { comp.value = Some(val.clone()); }
            if let Some(ref mfr) = manufacturer { comp.manufacturer = Some(mfr.clone()); }
            if let Some(ref part_num) = mpn { comp.mpn = Some(part_num.clone()); }
            components.insert(des, comp);
        }
    }
    
    Ok(components)
}

fn get_cell_as_string(cell: &Data) -> Option<String> {
    match cell {
        Data::String(s) => {
            let trimmed = s.trim();
            if trimmed.is_empty() { None } else { Some(trimmed.to_string()) }
        }
        Data::Int(i) => Some(i.to_string()),
        Data::Float(f) => Some(f.to_string()),
        Data::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

fn expand_designators(input: &str) -> Vec<String> {
    let mut list = Vec::new();
    
    let parts = input.split(|c| c == ',' || c == ';' || c == ' ')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty());
        
    for part in parts {
        if part.contains('-') {
            let range_parts: Vec<&str> = part.split('-').collect();
            if range_parts.len() == 2 {
                let start = range_parts[0].trim();
                let end = range_parts[1].trim();
                
                let start_prefix: String = start.chars().take_while(|c| c.is_alphabetic()).collect();
                let end_prefix: String = end.chars().take_while(|c| c.is_alphabetic()).collect();
                
                if start_prefix == end_prefix && !start_prefix.is_empty() {
                    let start_num_str: String = start.chars().skip_while(|c| c.is_alphabetic()).collect();
                    let end_num_str: String = end.chars().skip_while(|c| c.is_alphabetic()).collect();
                    
                    if let (Ok(s_num), Ok(e_num)) = (start_num_str.parse::<i32>(), end_num_str.parse::<i32>()) {
                        for num in s_num..=e_num {
                            list.push(format!("{}{}", start_prefix, num));
                        }
                        continue;
                    }
                }
            }
        }
        list.push(part.to_string());
    }
    
    list
}
