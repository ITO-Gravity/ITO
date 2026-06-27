use std::path::Path;
use anyhow::{Result, Context};
use crate::models::{HardwareDesign, Component, Pin, ElectricalType};

/// Parsea un archivo de diseño KiCad (.kicad_pcb o .kicad_sch).
/// Soporta formato de S-expressions para KiCad v6+ y tiene fallback de lectura
/// de texto para compatibilidad con esquemas heredados.
pub fn parse_kicad<P: AsRef<Path>>(path: P) -> Result<HardwareDesign> {
    let content = std::fs::read_to_string(&path)
        .context("Error al abrir el archivo KiCad")?;
    
    let mut design = HardwareDesign::new();
    
    // 1. Intentar parseo de S-expressions (KiCad v6+)
    if let Ok(value) = lexpr::from_str(&content) {
        let list = cons_to_vec(&value);
        for item in list {
            let item_list = cons_to_vec(item);
            let first_sym = item_list.first().and_then(|v| v.as_symbol());
            
            if first_sym == Some("footprint") || first_sym == Some("symbol") {
                // Es un componente (Huella o Símbolo)
                let mut designator = String::new();
                let mut footprint = String::new();
                
                // Si es footprint, la firma/package suele ser el segundo término (ej. (footprint "Capacitor_SMD:C_0805"))
                if first_sym == Some("footprint") {
                    if let Some(fp_val) = item_list.get(1).and_then(|v| v.as_str()) {
                        footprint = fp_val.to_string();
                    }
                }

                // Buscar designador y pines en los hijos de la S-expression
                for node in item_list {
                    let node_list = cons_to_vec(node);
                    let node_sym = node_list.first().and_then(|v| v.as_symbol());
                    
                    if node_sym == Some("property") {
                        // Propiedades en esquemáticos v6+: (property "Reference" "R1" ...)
                        let prop_name = node_list.get(1).and_then(|v| v.as_str());
                        let prop_val = node_list.get(2).and_then(|v| v.as_str());
                        if prop_name == Some("Reference") {
                            designator = prop_val.unwrap_or("").to_string();
                        }
                    } else if node_sym == Some("descr") {
                        // Opcional en footprints legacy
                    } else if node_sym == Some("pad") {
                        // Pad físico de PCB: (pad "1" smd rect (at -1 0) (size 1 1) (nets ...))
                        let pad_id_cell = node_list.get(1);
                        let pad_id = if let Some(v) = pad_id_cell {
                            if v.is_string() { 
                                v.as_str().unwrap_or("").to_string() 
                            } else if v.is_u64() { 
                                v.as_u64().unwrap_or(0).to_string() 
                            } else if v.is_i64() { 
                                v.as_i64().unwrap_or(0).to_string() 
                            } else {
                                "".to_string()
                            }
                        } else {
                            "".to_string()
                        };
                        
                        if !pad_id.is_empty() && !designator.is_empty() {
                            if let Some(comp) = design.components.get_mut(&designator) {
                                comp.pins.insert(pad_id.clone(), Pin::new(&pad_id, &pad_id, ElectricalType::Passive));
                            }
                        }
                    }
                }
                
                if !designator.is_empty() {
                    let comp = Component::new(&designator, &footprint);
                    design.add_component(comp);
                }
            }
        }
    }

    // 2. Fallback robusto por escaneo de texto si no se extrajeron componentes
    // Esto da soporte a los formatos legacy de esquemas y PCBs v5
    if design.components.is_empty() {
        let mut current_designator = String::new();
        let mut current_footprint = String::new();

        for line in content.lines() {
            let trimmed = line.trim();
            // Buscar definiciones de componentes en esquemáticos legacy
            if trimmed.starts_with("$Comp") {
                current_designator = String::new();
            } else if trimmed.starts_with("L ") && !current_designator.is_empty() {
                // Librería del componente
            } else if trimmed.starts_with("F 0 ") {
                // Referencia de diseño (Designator)
                let parts: Vec<&str> = trimmed.split('"').collect();
                if parts.len() >= 2 {
                    current_designator = parts[1].to_string();
                }
            } else if trimmed.starts_with("F 2 ") {
                // Huella (Footprint)
                let parts: Vec<&str> = trimmed.split('"').collect();
                if parts.len() >= 2 {
                    current_footprint = parts[1].to_string();
                }
            } else if trimmed.starts_with("$EndComp") {
                if !current_designator.is_empty() {
                    let comp = Component::new(&current_designator, &current_footprint);
                    design.add_component(comp);
                }
                current_designator = String::new();
                current_footprint = String::new();
            }
        }
    }

    Ok(design)
}

fn cons_to_vec(mut val: &lexpr::Value) -> Vec<&lexpr::Value> {
    let mut vec = Vec::new();
    while let Some(cons) = val.as_cons() {
        vec.push(cons.car());
        val = cons.cdr();
    }
    vec
}
