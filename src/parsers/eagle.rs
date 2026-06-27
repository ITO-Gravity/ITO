use std::path::Path;
use anyhow::{Result, Context};
use crate::models::{HardwareDesign, Component, Pin, ElectricalType};

/// Parsea archivos XML de Autodesk Eagle (.sch o .brd).
/// Extrae componentes de las etiquetas <part> y conexiones de <net>/<signal>.
pub fn parse_eagle<P: AsRef<Path>>(path: P) -> Result<HardwareDesign> {
    let content = std::fs::read_to_string(&path)
        .context("Error al abrir el archivo Eagle XML")?;
        
    let doc = roxmltree::Document::parse(&content)
        .context("Error al parsear el formato XML de Eagle")?;
        
    let mut design = HardwareDesign::new();
    
    // 1. Extraer componentes de <parts> (esquemático) o <elements> (PCB)
    for node in doc.descendants() {
        if node.has_tag_name("part") || node.has_tag_name("element") {
            let name = node.attribute("name").unwrap_or("");
            let value = node.attribute("value").unwrap_or("");
            let footprint = node.attribute("package").unwrap_or("");
            let deviceset = node.attribute("deviceset").unwrap_or("");
            
            if !name.is_empty() {
                let mut comp = Component::new(name, footprint);
                if !value.is_empty() {
                    comp.value = Some(value.to_string());
                }
                if !deviceset.is_empty() {
                    comp.mpn = Some(deviceset.to_string());
                }
                design.add_component(comp);
            }
        }
    }
    
    // 2. Extraer redes y conexiones de <nets> (esquemático) o <signals> (PCB)
    for node in doc.descendants() {
        if node.has_tag_name("net") || node.has_tag_name("signal") {
            let net_name = node.attribute("name").unwrap_or("GENERIC_NET");
            
            for child in node.descendants() {
                if child.has_tag_name("pinref") || child.has_tag_name("contactref") {
                    let part = child.attribute("part").unwrap_or(child.attribute("element").unwrap_or(""));
                    let pin = child.attribute("pin").unwrap_or(child.attribute("pad").unwrap_or(""));
                    
                    if !part.is_empty() && !pin.is_empty() {
                        design.add_net_endpoint(net_name, part, pin);
                        
                        if let Some(comp) = design.components.get_mut(part) {
                            if !comp.pins.contains_key(pin) {
                                comp.pins.insert(pin.to_string(), Pin::new(pin, pin, ElectricalType::Unspecified));
                            }
                        }
                    }
                }
            }
        }
    }
    
    Ok(design)
}
