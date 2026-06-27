use std::collections::{HashMap, HashSet};
use crate::models::{Component, Pin, ElectricalType, Net, PinReference, HardwareDesign};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PinChange {
    Name { old: String, new: String },
    ElectricalType { old: ElectricalType, new: ElectricalType },
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ComponentChange {
    Mpn { old: Option<String>, new: Option<String> },
    Manufacturer { old: Option<String>, new: Option<String> },
    Value { old: Option<String>, new: Option<String> },
    Footprint { old: Option<String>, new: Option<String> },
    PinAdded { pin_id: String, pin_name: String },
    PinDeleted { pin_id: String, pin_name: String },
    PinModified { pin_id: String, changes: Vec<PinChange> },
    AttributeAdded { key: String, val: String },
    AttributeDeleted { key: String, val: String },
    AttributeModified { key: String, old: String, new: String },
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ComponentDiff {
    pub old: Component,
    pub new: Component,
    pub changes: Vec<ComponentChange>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ComponentDiffs {
    pub added: HashMap<String, Component>,
    pub deleted: HashMap<String, Component>,
    pub modified: HashMap<String, ComponentDiff>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct NetDiff {
    pub old: Net,
    pub new: Net,
    pub added_endpoints: HashSet<PinReference>,
    pub deleted_endpoints: HashSet<PinReference>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct NetDiffs {
    pub added: HashMap<String, Net>,
    pub deleted: HashMap<String, Net>,
    pub modified: HashMap<String, NetDiff>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DesignDiff {
    pub components: ComponentDiffs,
    pub nets: NetDiffs,
}

impl DesignDiff {
    pub fn is_empty(&self) -> bool {
        self.components.added.is_empty()
            && self.components.deleted.is_empty()
            && self.components.modified.is_empty()
            && self.nets.added.is_empty()
            && self.nets.deleted.is_empty()
            && self.nets.modified.is_empty()
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ItoReport {
    pub project_id: String,
    pub domain: String,
    pub timestamp: String,
    pub ito_version: String,
    pub diff: DesignDiff,
}

impl ItoReport {
    pub fn new(project_id: String, diff: DesignDiff) -> Self {
        let timestamp = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        Self {
            project_id,
            domain: "hardware".to_string(),
            timestamp,
            ito_version: env!("CARGO_PKG_VERSION").to_string(),
            diff,
        }
    }
}


/// Realiza una comparación semántica entre un diseño antiguo (`old`) y uno nuevo (`new`).
/// Detecta componentes agregados, eliminados o modificados, y redes eléctricas con sus conexiones alteradas.
pub fn diff_designs(old: &HardwareDesign, new: &HardwareDesign) -> DesignDiff {
    let mut added_components = HashMap::new();
    let mut deleted_components = HashMap::new();
    let mut modified_components = HashMap::new();

    // 1. Detección de componentes agregados y modificados
    for (designator, new_comp) in &new.components {
        if let Some(old_comp) = old.components.get(designator) {
            let changes = diff_component(old_comp, new_comp);
            if !changes.is_empty() {
                modified_components.insert(
                    designator.clone(),
                    ComponentDiff {
                        old: old_comp.clone(),
                        new: new_comp.clone(),
                        changes,
                    },
                );
            }
        } else {
            added_components.insert(designator.clone(), new_comp.clone());
        }
    }

    // 2. Detección de componentes eliminados
    for (designator, old_comp) in &old.components {
        if !new.components.contains_key(designator) {
            deleted_components.insert(designator.clone(), old_comp.clone());
        }
    }

    let mut added_nets = HashMap::new();
    let mut deleted_nets = HashMap::new();
    let mut modified_nets = HashMap::new();

    // 3. Detección de nets agregadas y modificadas
    for (net_name, new_net) in &new.nets {
        if let Some(old_net) = old.nets.get(net_name) {
            let added_endpoints: HashSet<PinReference> = new_net
                .endpoints
                .difference(&old_net.endpoints)
                .cloned()
                .collect();
            let deleted_endpoints: HashSet<PinReference> = old_net
                .endpoints
                .difference(&new_net.endpoints)
                .cloned()
                .collect();

            if !added_endpoints.is_empty() || !deleted_endpoints.is_empty() {
                modified_nets.insert(
                    net_name.clone(),
                    NetDiff {
                        old: old_net.clone(),
                        new: new_net.clone(),
                        added_endpoints,
                        deleted_endpoints,
                    },
                );
            }
        } else {
            added_nets.insert(net_name.clone(), new_net.clone());
        }
    }

    // 4. Detección de nets eliminadas
    for (net_name, old_net) in &old.nets {
        if !new.nets.contains_key(net_name) {
            deleted_nets.insert(net_name.clone(), old_net.clone());
        }
    }

    DesignDiff {
        components: ComponentDiffs {
            added: added_components,
            deleted: deleted_components,
            modified: modified_components,
        },
        nets: NetDiffs {
            added: added_nets,
            deleted: deleted_nets,
            modified: modified_nets,
        },
    }
}

/// Compara dos componentes y extrae una lista detallada de sus diferencias semánticas.
fn diff_component(old: &Component, new: &Component) -> Vec<ComponentChange> {
    let mut changes = Vec::new();

    if old.mpn != new.mpn {
        changes.push(ComponentChange::Mpn {
            old: old.mpn.clone(),
            new: new.mpn.clone(),
        });
    }

    if old.manufacturer != new.manufacturer {
        changes.push(ComponentChange::Manufacturer {
            old: old.manufacturer.clone(),
            new: new.manufacturer.clone(),
        });
    }

    if old.value != new.value {
        changes.push(ComponentChange::Value {
            old: old.value.clone(),
            new: new.value.clone(),
        });
    }

    if old.footprint != new.footprint {
        changes.push(ComponentChange::Footprint {
            old: old.footprint.clone(),
            new: new.footprint.clone(),
        });
    }

    // Comparar pines
    for (pin_id, new_pin) in &new.pins {
        if let Some(old_pin) = old.pins.get(pin_id) {
            let mut pin_changes = Vec::new();
            if old_pin.name != new_pin.name {
                pin_changes.push(PinChange::Name {
                    old: old_pin.name.clone(),
                    new: new_pin.name.clone(),
                });
            }
            if old_pin.electrical_type != new_pin.electrical_type {
                pin_changes.push(PinChange::ElectricalType {
                    old: old_pin.electrical_type.clone(),
                    new: new_pin.electrical_type.clone(),
                });
            }
            if !pin_changes.is_empty() {
                changes.push(ComponentChange::PinModified {
                    pin_id: pin_id.clone(),
                    changes: pin_changes,
                });
            }
        } else {
            changes.push(ComponentChange::PinAdded {
                pin_id: pin_id.clone(),
                pin_name: new_pin.name.clone(),
            });
        }
    }

    for (pin_id, old_pin) in &old.pins {
        if !new.pins.contains_key(pin_id) {
            changes.push(ComponentChange::PinDeleted {
                pin_id: pin_id.clone(),
                pin_name: old_pin.name.clone(),
            });
        }
    }

    // Comparar atributos extra
    for (k, new_val) in &new.attributes {
        if let Some(old_val) = old.attributes.get(k) {
            if old_val != new_val {
                changes.push(ComponentChange::AttributeModified {
                    key: k.clone(),
                    old: old_val.clone(),
                    new: new_val.clone(),
                });
            }
        } else {
            changes.push(ComponentChange::AttributeAdded {
                key: k.clone(),
                val: new_val.clone(),
            });
        }
    }

    for (k, old_val) in &old.attributes {
        if !new.attributes.contains_key(k) {
            changes.push(ComponentChange::AttributeDeleted {
                key: k.clone(),
                val: old_val.clone(),
            });
        }
    }

    changes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::PinReference;

    #[test]
    fn test_diff_designs_component_footprint_change_and_net_expansion() {
        // --- 1. Crear el diseño viejo (OLD) ---
        let mut old_design = HardwareDesign::new();
        
        let old_r1 = Component::new("R1", "0603")
            .with_details("MC0603F103", "Multicomp", "10k")
            .with_pin(Pin::new("1", "P1", ElectricalType::Passive))
            .with_pin(Pin::new("2", "P2", ElectricalType::Passive));

        let old_u1 = Component::new("U1", "SOIC-8")
            .with_pin(Pin::new("1", "GND", ElectricalType::PowerInput))
            .with_pin(Pin::new("8", "VCC", ElectricalType::PowerInput));

        old_design.add_component(old_r1);
        old_design.add_component(old_u1);
        
        // Net inicial con una sola conexión (o estado básico)
        old_design.add_net_endpoint("3V3", "U1", "8");

        // --- 2. Crear el diseño nuevo (NEW) ---
        let mut new_design = HardwareDesign::new();
        
        // R1 muta su footprint de 0603 a 1206 y cambia su MPN
        let new_r1 = Component::new("R1", "1206")
            .with_details("MC1206F103", "Multicomp", "10k")
            .with_pin(Pin::new("1", "P1", ElectricalType::Passive))
            .with_pin(Pin::new("2", "P2", ElectricalType::Passive));

        let new_u1 = Component::new("U1", "SOIC-8")
            .with_pin(Pin::new("1", "GND", ElectricalType::PowerInput))
            .with_pin(Pin::new("8", "VCC", ElectricalType::PowerInput));

        // Añadimos un capacitor C1 completamente nuevo
        let new_c1 = Component::new("C1", "0805")
            .with_details("MC0805C104", "Multicomp", "100nF")
            .with_pin(Pin::new("1", "P1", ElectricalType::Passive))
            .with_pin(Pin::new("2", "P2", ElectricalType::Passive));

        new_design.add_component(new_r1);
        new_design.add_component(new_u1);
        new_design.add_component(new_c1);

        // La Net "3V3" gana conexiones (se conecta R1:2 y C1:1)
        new_design.add_net_endpoint("3V3", "U1", "8");
        new_design.add_net_endpoint("3V3", "R1", "2");
        new_design.add_net_endpoint("3V3", "C1", "1");

        // --- 3. Ejecutar comparación ---
        let diff = diff_designs(&old_design, &new_design);

        // --- 4. Aserciones ---
        // Aserciones de componentes
        assert_eq!(diff.components.added.len(), 1);
        assert!(diff.components.added.contains_key("C1"));
        assert_eq!(diff.components.deleted.len(), 0);
        
        assert_eq!(diff.components.modified.len(), 1);
        assert!(diff.components.modified.contains_key("R1"));
        
        let r1_diff = diff.components.modified.get("R1").unwrap();
        assert_eq!(r1_diff.changes.len(), 2);
        
        // Verificar cambios de footprint y MPN de R1
        let has_footprint_change = r1_diff.changes.iter().any(|c| {
            matches!(c, ComponentChange::Footprint { old: Some(ref o), new: Some(ref n) } if o == "0603" && n == "1206")
        });
        let has_mpn_change = r1_diff.changes.iter().any(|c| {
            matches!(c, ComponentChange::Mpn { old: Some(ref o), new: Some(ref n) } if o == "MC0603F103" && n == "MC1206F103")
        });
        assert!(has_footprint_change);
        assert!(has_mpn_change);

        // Aserciones de nets
        assert_eq!(diff.nets.added.len(), 0);
        assert_eq!(diff.nets.deleted.len(), 0);
        assert_eq!(diff.nets.modified.len(), 1);
        
        let net_diff = diff.nets.modified.get("3V3").unwrap();
        assert_eq!(net_diff.added_endpoints.len(), 2);
        assert_eq!(net_diff.deleted_endpoints.len(), 0);
        
        assert!(net_diff.added_endpoints.contains(&PinReference {
            component_designator: "R1".to_string(),
            pin_id: "2".to_string(),
        }));
        assert!(net_diff.added_endpoints.contains(&PinReference {
            component_designator: "C1".to_string(),
            pin_id: "1".to_string(),
        }));
    }

    #[test]
    fn test_ito_report_serialization() {
        let old_design = HardwareDesign::new();
        let new_design = HardwareDesign::new();
        let diff = diff_designs(&old_design, &new_design);
        
        let report = ItoReport::new("test-project-123".to_string(), diff);
        let json_str = serde_json::to_string(&report).unwrap();
        
        let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        
        assert_eq!(parsed["project_id"], "test-project-123");
        assert_eq!(parsed["domain"], "hardware");
        assert!(parsed["timestamp"].is_string());
        assert!(parsed["ito_version"].is_string());
        assert!(parsed["diff"].is_object());
    }
}
