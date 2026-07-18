use std::collections::HashMap;
use std::fs::File;
use std::io::BufReader;
use std::path::Path;
use anyhow::{Result, anyhow};

use crate::models::{Component, Pin, ElectricalType, HardwareDesign};

#[derive(Debug, serde::Deserialize)]
struct BomRow {
    #[serde(alias = "Designator", alias = "designator", alias = "RefDes")]
    designator: String,
    
    #[serde(alias = "MPN", alias = "mpn", alias = "PartNumber")]
    mpn: Option<String>,
    
    #[serde(alias = "Manufacturer", alias = "manufacturer", alias = "Brand")]
    manufacturer: Option<String>,
    
    #[serde(alias = "Value", alias = "value")]
    value: Option<String>,
    
    #[serde(alias = "Footprint", alias = "footprint", alias = "Package")]
    footprint: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
struct CadJson {
    components: Vec<CadComponent>,
    nets: Vec<CadNet>,
}

#[derive(Debug, serde::Deserialize)]
struct CadComponent {
    designator: String,
    footprint: Option<String>,
    pins: Vec<CadPin>,
}

#[derive(Debug, serde::Deserialize)]
struct CadPin {
    id: String,
    name: String,
    #[serde(alias = "electricalType", alias = "type")]
    electrical_type: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
struct CadNet {
    name: String,
    connections: Vec<CadConnection>,
}

#[derive(Debug, serde::Deserialize)]
struct CadConnection {
    component: String,
    pin: String,
}

/// Parsea un archivo de BOM en formato CSV desde un Reader.
pub fn parse_bom_csv_reader<R: std::io::Read>(reader: R) -> Result<HashMap<String, Component>> {
    let mut rdr = csv::ReaderBuilder::new()
        .flexible(true)
        .trim(csv::Trim::All)
        .from_reader(reader);
    
    let mut components = HashMap::new();
    
    for result in rdr.deserialize::<BomRow>() {
        let row = result?;
        
        // Soporte para designadores agrupados (ej. "R1, R2, R3" o "R1 R2 R3")
        let designators = row.designator
            .split(|c| c == ',' || c == ';'|| c == ' ')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty());
            
        for des in designators {
            let mut component = Component::new(des, row.footprint.as_deref().unwrap_or(""));
            if let Some(ref mpn) = row.mpn {
                component.mpn = Some(mpn.clone());
            }
            if let Some(ref mfr) = row.manufacturer {
                component.manufacturer = Some(mfr.clone());
            }
            if let Some(ref val) = row.value {
                component.value = Some(val.clone());
            }
            components.insert(des.to_string(), component);
        }
    }
    
    Ok(components)
}

pub mod kicad;
pub mod eagle;
pub mod excel_bom;
pub mod edif;

/// Parsea una lista de materiales (BOM) en formato CSV o Excel.
pub fn parse_bom<P: AsRef<Path>>(path: P) -> Result<HashMap<String, Component>> {
    let ext = path.as_ref().extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_lowercase();
    
    match ext.as_str() {
        "xlsx" | "xls" | "ods" => excel_bom::parse_excel_bom(path),
        _ => parse_bom_csv(path),
    }
}

/// Parsea un archivo de diseño CAD (JSON, KiCad, Eagle).
pub fn parse_cad<P: AsRef<Path>>(path: P) -> Result<HardwareDesign> {
    let ext = path.as_ref().extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_lowercase();
    
    match ext.as_str() {
        "kicad_pcb" | "kicad_sch" => kicad::parse_kicad(path),
        "edif" | "edf" => edif::parse_edif(path),
        "sch" | "brd" => {
            if is_xml_file(&path) {
                eagle::parse_eagle(path)
            } else {
                Err(anyhow!("Formato de esquema/PCB no soportado"))
            }
        }
        _ => parse_cad_json(path),
    }
}

fn is_xml_file<P: AsRef<Path>>(path: P) -> bool {
    if let Ok(content) = std::fs::read_to_string(path) {
        let trimmed = content.trim_start();
        trimmed.starts_with("<?xml") || trimmed.starts_with("<eagle")
    } else {
        false
    }
}

/// Escanea un directorio de proyecto, localiza y parsea automáticamente
/// cualquier archivo nativo de diseño de hardware y su BOM correspondiente,
/// unificándolos en un HardwareDesign.
pub fn parse_project_directory<P: AsRef<Path>>(dir: P) -> Result<HardwareDesign> {
    let dir = dir.as_ref();
    
    let mut cad_file = None;
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() {
                if let Some(ext) = path.extension().and_then(|s| s.to_str()) {
                    let ext_lower = ext.to_lowercase();
                    if ext_lower == "kicad_pcb" || ext_lower == "kicad_sch" || ext_lower == "brd" || ext_lower == "edif" || ext_lower == "edf" || (ext_lower == "sch" && !path.to_string_lossy().contains("bom")) {
                        cad_file = Some(path);
                        break;
                    }
                }
            }
        }
    }
    
    let cad_path = cad_file.unwrap_or_else(|| dir.join("design.json"));
    if !cad_path.exists() {
        return Err(anyhow!("No se encontró ningún archivo de diseño de hardware (design.json, .kicad_pcb, .sch, .brd, .edif)"));
    }
    
    let mut design = parse_cad(&cad_path)?;
    
    let mut bom_file = None;
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() {
                if let Some(ext) = path.extension().and_then(|s| s.to_str()) {
                    let ext_lower = ext.to_lowercase();
                    if ext_lower == "xlsx" || ext_lower == "xls" || (ext_lower == "csv" && path.file_name().and_then(|s| s.to_str()).unwrap_or("").to_lowercase().contains("bom")) {
                        bom_file = Some(path);
                        break;
                    }
                }
            }
        }
    }
    
    let bom_path = bom_file.unwrap_or_else(|| dir.join("bom.csv"));
    if bom_path.exists() {
        let bom = parse_bom(&bom_path)?;
        design.merge_bom(bom);
    }
    
    Ok(design)
}

/// Indica si un directorio contiene un archivo fuente de diseño de hardware reconocible
/// (CAD nativo o `design.json`). Permite distinguir "no hay diseño" (vacío legítimo) de
/// "hay un diseño pero no se pudo parsear" (corrupto), para no tratar la corrupción como vacío.
pub fn has_design_source<P: AsRef<Path>>(dir: P) -> bool {
    let dir = dir.as_ref();
    if dir.join("design.json").is_file() {
        return true;
    }
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() {
                if let Some(ext) = path.extension().and_then(|s| s.to_str()) {
                    let e = ext.to_lowercase();
                    if e == "kicad_pcb" || e == "kicad_sch" || e == "brd" || e == "edif" || e == "edf"
                        || (e == "sch" && !path.to_string_lossy().to_lowercase().contains("bom"))
                    {
                        return true;
                    }
                }
            }
        }
    }
    false
}

/// Parsea un archivo de BOM en formato CSV desde una ruta del sistema.
pub fn parse_bom_csv<P: AsRef<Path>>(path: P) -> Result<HashMap<String, Component>> {
    let file = File::open(path)?;
    parse_bom_csv_reader(BufReader::new(file))
}

/// Parsea un archivo de CAD en formato JSON (como Flux) desde un Reader.
pub fn parse_cad_json_reader<R: std::io::Read>(reader: R) -> Result<HardwareDesign> {
    let cad: CadJson = serde_json::from_reader(reader)?;
    let mut design = HardwareDesign::new();
    
    for c in cad.components {
        let mut comp = Component::new(&c.designator, c.footprint.as_deref().unwrap_or(""));
        for p in c.pins {
            let elec_type = if let Some(ref et_str) = p.electrical_type {
                et_str.parse().unwrap_or(ElectricalType::Unspecified)
            } else {
                ElectricalType::Unspecified
            };
            let pin = Pin::new(&p.id, &p.name, elec_type);
            comp = comp.with_pin(pin);
        }
        design.add_component(comp);
    }
    
    for n in cad.nets {
        for conn in n.connections {
            design.add_net_endpoint(&n.name, &conn.component, &conn.pin);
        }
    }
    
    Ok(design)
}

/// Parsea un archivo de CAD en formato JSON desde una ruta del sistema.
pub fn parse_cad_json<P: AsRef<Path>>(path: P) -> Result<HardwareDesign> {
    let file = File::open(path)?;
    parse_cad_json_reader(BufReader::new(file))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::PinReference;

    #[test]
    fn test_parse_bom_csv() {
        let csv_data = r#"Designator,MPN,Manufacturer,Value,Footprint
"R1, R2",MC0805F103,Multicomp,10k,0805
U1,NE555P,TI,555 Timer,SOIC-8
"#;

        let result = parse_bom_csv_reader(csv_data.as_bytes()).unwrap();

        assert_eq!(result.len(), 3);
        assert!(result.contains_key("R1"));
        assert!(result.contains_key("R2"));
        assert!(result.contains_key("U1"));

        let r1 = result.get("R1").unwrap();
        assert_eq!(r1.value.as_deref(), Some("10k"));
        assert_eq!(r1.manufacturer.as_deref(), Some("Multicomp"));
        assert_eq!(r1.footprint.as_deref(), Some("0805"));

        let u1 = result.get("U1").unwrap();
        assert_eq!(u1.mpn.as_deref(), Some("NE555P"));
        assert_eq!(u1.footprint.as_deref(), Some("SOIC-8"));
    }

    #[test]
    fn test_parse_cad_json() {
        let json_data = r#"{
          "components": [
            {
              "designator": "U1",
              "footprint": "SOIC-8",
              "pins": [
                {"id": "1", "name": "GND", "electricalType": "PowerInput"},
                {"id": "8", "name": "VCC", "electricalType": "PowerInput"}
              ]
            },
            {
              "designator": "R1",
              "footprint": "0805",
              "pins": [
                {"id": "1", "name": "P1", "type": "Passive"},
                {"id": "2", "name": "P2", "type": "Passive"}
              ]
            }
          ],
          "nets": [
            {
              "name": "SPI_MISO",
              "connections": [
                {"component": "U1", "pin": "1"},
                {"component": "R1", "pin": "2"}
              ]
            }
          ]
        }"#;

        let design = parse_cad_json_reader(json_data.as_bytes()).unwrap();

        assert_eq!(design.components.len(), 2);
        assert!(design.components.contains_key("U1"));
        assert!(design.components.contains_key("R1"));

        let u1 = design.components.get("U1").unwrap();
        assert_eq!(u1.pins.len(), 2);
        assert_eq!(u1.pins.get("1").unwrap().electrical_type, ElectricalType::PowerInput);

        assert!(design.nets.contains_key("SPI_MISO"));
        let net = design.nets.get("SPI_MISO").unwrap();
        assert_eq!(net.endpoints.len(), 2);
        assert!(net.endpoints.contains(&PinReference {
            component_designator: "U1".to_string(),
            pin_id: "1".to_string(),
        }));
    }

    #[test]
    fn test_merge_bom_and_cad() {
        // 1. JSON del CAD (Estructura física y conexiones)
        let json_data = r#"{
          "components": [
            {
              "designator": "R1",
              "footprint": "0805",
              "pins": [
                {"id": "1", "name": "P1"},
                {"id": "2", "name": "P2"}
              ]
            }
          ],
          "nets": []
        }"#;

        let mut design = parse_cad_json_reader(json_data.as_bytes()).unwrap();
        
        // Antes del merge, no tenemos datos comerciales/de fabricación
        let r1_cad = design.components.get("R1").unwrap();
        assert_eq!(r1_cad.mpn, None);
        assert_eq!(r1_cad.manufacturer, None);

        // 2. CSV de la BOM (Datos comerciales)
        let csv_data = r#"Designator,MPN,Manufacturer,Value,Footprint
R1,MC0805F103,Multicomp,10k,0805
"#;
        let bom = parse_bom_csv_reader(csv_data.as_bytes()).unwrap();

        // 3. Fusionar datos de la BOM en el diseño CAD
        let (merged, missing) = design.merge_bom(bom);

        assert_eq!(merged, 1);
        assert_eq!(missing.len(), 0);

        // Después del merge, U1 en CAD tiene la información de la BOM
        let r1_merged = design.components.get("R1").unwrap();
        assert_eq!(r1_merged.mpn.as_deref(), Some("MC0805F103"));
        assert_eq!(r1_merged.manufacturer.as_deref(), Some("Multicomp"));
        assert_eq!(r1_merged.value.as_deref(), Some("10k"));
        assert_eq!(r1_merged.footprint.as_deref(), Some("0805"));
    }

    #[test]
    fn test_parse_edif() {
        let edif_data = r#"(edif test_design
          (edifVersion 2 0 0)
          (edifLevel 0)
          (keywordMap (keywordLevel 0))
          (library lib
            (cell U1
              (view view_1
                (interface
                  (port p1)
                )
              )
            )
          )
          (design root
            (cellRef U1 (libraryRef lib))
            (contents
              (instance R1)
              (net net_vcc
                (joined
                  (portRef p1 (instanceRef R1))
                )
              )
            )
          )
        )"#;
        
        let mut design = crate::models::HardwareDesign::new();
        if let Ok(value) = lexpr::from_str(edif_data) {
            super::edif::traverse_edif_node(&value, &mut design);
        }
        
        assert!(design.components.contains_key("R1"));
        let net = design.nets.get("net_vcc").unwrap();
        assert_eq!(net.endpoints.len(), 1);
        assert!(net.endpoints.contains(&crate::models::PinReference {
            component_designator: "R1".to_string(),
            pin_id: "p1".to_string(),
        }));
    }
}
