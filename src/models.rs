use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ElectricalType {
    Input,
    Output,
    Bidirectional,
    PowerInput,
    PowerOutput,
    Passive,
    OpenCollector,
    TriState,
    Unspecified,
}

impl std::str::FromStr for ElectricalType {
    type Err = ();

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_lowercase().replace('_', "").as_str() {
            "input" => Ok(ElectricalType::Input),
            "output" => Ok(ElectricalType::Output),
            "bidirectional" => Ok(ElectricalType::Bidirectional),
            "powerinput" | "power" => Ok(ElectricalType::PowerInput),
            "poweroutput" => Ok(ElectricalType::PowerOutput),
            "passive" => Ok(ElectricalType::Passive),
            "opencollector" => Ok(ElectricalType::OpenCollector),
            "tristate" => Ok(ElectricalType::TriState),
            _ => Ok(ElectricalType::Unspecified),
        }
    }
}


#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Pin {
    pub id: String, // ej. "1", "A2"
    pub name: String, // ej. "MISO", "VCC"
    pub electrical_type: ElectricalType,
}

impl Pin {
    pub fn new(id: &str, name: &str, electrical_type: ElectricalType) -> Self {
        Self {
            id: id.to_string(),
            name: name.to_string(),
            electrical_type,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Component {
    pub designator: String, // ej. "U1", "R5"
    pub mpn: Option<String>, // Manufacturer Part Number
    pub manufacturer: Option<String>,
    pub value: Option<String>, // ej. "10k", "100nF"
    pub footprint: Option<String>, // ej. "SOIC-8", "0805"
    pub pins: HashMap<String, Pin>, // id -> Pin
    pub attributes: HashMap<String, String>, // metadatos extra
}

impl Component {
    pub fn new(designator: &str, footprint: &str) -> Self {
        Self {
            designator: designator.to_string(),
            mpn: None,
            manufacturer: None,
            value: None,
            footprint: Some(footprint.to_string()),
            pins: HashMap::new(),
            attributes: HashMap::new(),
        }
    }

    #[allow(dead_code)]
    pub fn with_details(
        mut self,
        mpn: &str,
        manufacturer: &str,
        value: &str,
    ) -> Self {
        self.mpn = Some(mpn.to_string());
        self.manufacturer = Some(manufacturer.to_string());
        self.value = Some(value.to_string());
        self
    }

    pub fn with_pin(mut self, pin: Pin) -> Self {
        self.pins.insert(pin.id.clone(), pin);
        self
    }

    pub fn add_attribute(&mut self, key: &str, val: &str) {
        self.attributes.insert(key.to_string(), val.to_string());
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct PinReference {
    pub component_designator: String, // ej. "U1"
    pub pin_id: String, // ej. "4"
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Net {
    pub name: String, // ej. "SPI_CLK", "Net-(R1-Pad2)"
    pub endpoints: HashSet<PinReference>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct HardwareDesign {
    pub components: HashMap<String, Component>, // designator -> Component
    pub nets: HashMap<String, Net>, // net_name -> Net
}

impl HardwareDesign {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_component(&mut self, component: Component) {
        self.components.insert(component.designator.clone(), component);
    }

    pub fn add_net_endpoint(&mut self, net_name: &str, component_designator: &str, pin_id: &str) {
        let net = self.nets.entry(net_name.to_string()).or_insert_with(|| Net {
            name: net_name.to_string(),
            endpoints: HashSet::new(),
        });
        net.endpoints.insert(PinReference {
            component_designator: component_designator.to_string(),
            pin_id: pin_id.to_string(),
        });
    }

    /// Fusiona la información de una BOM (HashMap de componentes) en el diseño actual.
    /// Si un componente de la BOM coincide por designator, actualiza sus campos de fabricación.
    /// Retorna una tupla con (cantidad_fusionados, designators_no_encontrados_en_cad).
    pub fn merge_bom(&mut self, bom: HashMap<String, Component>) -> (usize, Vec<String>) {
        let mut merged_count = 0;
        let mut missing_in_cad = Vec::new();

        for (des, bom_comp) in bom {
            if let Some(cad_comp) = self.components.get_mut(&des) {
                if bom_comp.mpn.is_some() {
                    cad_comp.mpn = bom_comp.mpn;
                }
                if bom_comp.manufacturer.is_some() {
                    cad_comp.manufacturer = bom_comp.manufacturer;
                }
                if bom_comp.value.is_some() {
                    cad_comp.value = bom_comp.value;
                }
                if bom_comp.footprint.is_some() && (cad_comp.footprint.is_none() || cad_comp.footprint.as_deref() == Some("")) {
                    cad_comp.footprint = bom_comp.footprint;
                }
                // Fusionar atributos extra
                for (k, v) in bom_comp.attributes {
                    cad_comp.add_attribute(&k, &v);
                }
                merged_count += 1;
            } else {
                missing_in_cad.push(des);
            }
        }

        (merged_count, missing_in_cad)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_component_builder() {
        let pin1 = Pin::new("1", "VCC", ElectricalType::PowerInput);
        let pin2 = Pin::new("2", "GND", ElectricalType::PowerOutput);
        
        let comp = Component::new("U1", "SOIC-8")
            .with_details("NE555P", "TI", "555 Timer")
            .with_pin(pin1.clone())
            .with_pin(pin2.clone());

        assert_eq!(comp.designator, "U1");
        assert_eq!(comp.footprint, Some("SOIC-8".to_string()));
        assert_eq!(comp.mpn, Some("NE555P".to_string()));
        assert_eq!(comp.pins.len(), 2);
        assert_eq!(comp.pins.get("1"), Some(&pin1));
        assert_eq!(comp.pins.get("2"), Some(&pin2));
    }

    #[test]
    fn test_hardware_design_connections() {
        let mut design = HardwareDesign::new();

        let r1 = Component::new("R1", "0805")
            .with_details("MC0805F103", "Multicomp", "10k")
            .with_pin(Pin::new("1", "P1", ElectricalType::Passive))
            .with_pin(Pin::new("2", "P2", ElectricalType::Passive));

        let u1 = Component::new("U1", "SOIC-8")
            .with_pin(Pin::new("1", "GND", ElectricalType::PowerInput))
            .with_pin(Pin::new("8", "VCC", ElectricalType::PowerInput));

        design.add_component(r1);
        design.add_component(u1);

        // Conectar pin R1:2 a U1:8 en la red "3V3"
        design.add_net_endpoint("3V3", "R1", "2");
        design.add_net_endpoint("3V3", "U1", "8");

        assert_eq!(design.components.len(), 2);
        assert!(design.nets.contains_key("3V3"));

        let net_3v3 = design.nets.get("3V3").unwrap();
        assert_eq!(net_3v3.endpoints.len(), 2);
        assert!(net_3v3.endpoints.contains(&PinReference {
            component_designator: "R1".to_string(),
            pin_id: "2".to_string(),
        }));
        assert!(net_3v3.endpoints.contains(&PinReference {
            component_designator: "U1".to_string(),
            pin_id: "8".to_string(),
        }));
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ItoProjectModules {
    pub firmware: bool,
    pub electronics: bool,
    pub mechanical: bool,
    pub documentation: bool,
    pub manufacturing: bool,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ItoProjectLink {
    pub path: String,
    pub tool: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ItoProjectConfig {
    pub format_version: String,
    pub project_name: String,
    pub project_uuid: String,
    pub created_at: String,
    pub created_by: String,
    pub modules: ItoProjectModules,
    pub current_revision: String,
    pub license: String,
    pub version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub links: Option<std::collections::HashMap<String, ItoProjectLink>>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ItoWorkspaceConfig {
    pub workspace: String,
    pub version: String,
}
