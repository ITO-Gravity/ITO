use std::path::Path;
use anyhow::{Result, Context};
use crate::models::{HardwareDesign, Component, Pin, ElectricalType};

/// Parsea un archivo de red EDIF (.edif o .edf) utilizando S-expressions.
/// Extrae componentes de las instancias y conexiones de las uniones net.
pub fn parse_edif<P: AsRef<Path>>(path: P) -> Result<HardwareDesign> {
    let content = std::fs::read_to_string(&path)
        .context("Error al abrir el archivo EDIF")?;
        
    let mut design = HardwareDesign::new();
    
    // 1. Intentar parseo de S-expressions
    if let Ok(value) = lexpr::from_str(&content) {
        traverse_edif_node(&value, &mut design);
    }
    
    // 2. Fallback de texto simple por línea si no se encontraron componentes
    if design.components.is_empty() {
        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("(instance ") {
                let parts: Vec<&str> = trimmed.split_whitespace().collect();
                if parts.len() >= 2 {
                    let name = parts[1].trim_matches(|c| c == '(' || c == ')' || c == '"');
                    let comp = Component::new(name, "");
                    design.add_component(comp);
                }
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

pub(crate) fn traverse_edif_node(node: &lexpr::Value, design: &mut HardwareDesign) {
    let list = cons_to_vec(node);
    if let Some(first_sym) = list.first().and_then(|v| v.as_symbol()) {
        if first_sym == "instance" {
            // Instancia de componente: (instance R1 (viewRef ...))
            if let Some(name_val) = list.get(1) {
                let name = get_value_name(name_val);
                if !name.is_empty() {
                    let comp = Component::new(&name, "");
                    design.add_component(comp);
                }
            }
        } else if first_sym == "net" {
            // Conexión net: (net VCC (joined (portRef 1 (instanceRef U1)) ...))
            if let Some(net_name_val) = list.get(1) {
                let net_name = get_value_name(net_name_val);
                
                for child in &list {
                    let child_list = cons_to_vec(child);
                    if child_list.first().and_then(|v| v.as_symbol()) == Some("joined") {
                        for port_node in &child_list {
                            let port_list = cons_to_vec(port_node);
                            if port_list.first().and_then(|v| v.as_symbol()) == Some("portRef") {
                                let pin_id = port_list.get(1).and_then(|v| get_value_name_opt(v)).unwrap_or_default();
                                
                                for p_child in &port_list {
                                    let p_child_list = cons_to_vec(p_child);
                                    if p_child_list.first().and_then(|v| v.as_symbol()) == Some("instanceRef") {
                                        let inst_name = p_child_list.get(1).and_then(|v| get_value_name_opt(v)).unwrap_or_default();
                                        if !inst_name.is_empty() && !pin_id.is_empty() {
                                            design.add_net_endpoint(&net_name, &inst_name, &pin_id);
                                            
                                            if let Some(comp) = design.components.get_mut(&inst_name) {
                                                if !comp.pins.contains_key(&pin_id) {
                                                    comp.pins.insert(pin_id.clone(), Pin::new(&pin_id, &pin_id, ElectricalType::Unspecified));
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    
    // Recorrer recursivamente
    for child in list {
        traverse_edif_node(child, design);
    }
}

fn get_value_name_opt(v: &lexpr::Value) -> Option<String> {
    if v.is_string() {
        v.as_str().map(|s| s.to_string())
    } else if v.is_symbol() {
        v.as_symbol().map(|s| s.to_string())
    } else if v.is_u64() {
        v.as_u64().map(|n| n.to_string())
    } else if v.is_i64() {
        v.as_i64().map(|n| n.to_string())
    } else {
        None
    }
}

fn get_value_name(v: &lexpr::Value) -> String {
    get_value_name_opt(v).unwrap_or_default()
}
