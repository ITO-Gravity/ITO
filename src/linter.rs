use crate::models::{HardwareDesign, ElectricalType};

#[derive(Debug, Clone, serde::Serialize)]
pub struct LintIssue {
    pub severity: LintSeverity,
    pub rule_id: String,
    pub message: String,
    pub details: String,
}

#[derive(Debug, Clone, serde::Serialize, PartialEq, Eq)]
pub enum LintSeverity {
    Critical,
    Warning,
    Info,
}

impl LintSeverity {
    pub fn as_str(&self) -> &'static str {
        match self {
            LintSeverity::Critical => "CRITICAL",
            LintSeverity::Warning => "WARNING",
            LintSeverity::Info => "INFO",
        }
    }
}

/// Ejecuta las reglas de diseño eléctrico semántico (ERC) sobre un diseño.
pub fn run_lint(design: &HardwareDesign) -> Vec<LintIssue> {
    let mut issues = Vec::new();
    
    // Mapear qué pins pertenecen a qué redes para consultas rápidas
    // net_name -> HashSet<PinReference>
    let mut net_pins = std::collections::HashMap::new();
    for (net_name, net) in &design.nets {
        net_pins.insert(net_name.clone(), &net.endpoints);
    }
    
    // Mapear pin de un componente a la red a la que pertenece
    // (component_designator, pin_id) -> net_name
    let mut pin_to_net = std::collections::HashMap::new();
    for (net_name, net) in &design.nets {
        for ep in &net.endpoints {
            pin_to_net.insert((ep.component_designator.clone(), ep.pin_id.clone()), net_name.clone());
        }
    }

    // Regla 1 y 3: Analizar componentes y sus pines individuales
    for (des, comp) in &design.components {
        let is_ic = des.starts_with('U') || comp.pins.len() > 3;
        
        for (pin_id, pin) in &comp.pins {
            let connected_net = pin_to_net.get(&(des.clone(), pin_id.clone()));
            
            match pin.electrical_type {
                ElectricalType::Input => {
                    // Regla 1: Pines de entrada flotantes
                    match connected_net {
                        None => {
                            issues.push(LintIssue {
                                severity: LintSeverity::Warning,
                                rule_id: "E001_FLOATING_INPUT".to_string(),
                                message: format!("Pin de entrada flotante en {}.{}", des, pin_id),
                                details: format!("El pin '{}' ({}) del componente '{}' está configurado como entrada pero no está conectado a ninguna red eléctrica.", pin.name, pin_id, des),
                            });
                        }
                        Some(net_name) => {
                            // Validar si la red tiene algún emisor/driver
                            if let Some(endpoints) = net_pins.get(net_name) {
                                let mut has_driver = false;
                                for ep in *endpoints {
                                    if let Some(ep_comp) = design.components.get(&ep.component_designator) {
                                        if let Some(ep_pin) = ep_comp.pins.get(&ep.pin_id) {
                                            // Un driver es cualquier pin de salida, bidireccional, colector abierto, tri-state o pasivo (ej. resistencias pull-up)
                                            match ep_pin.electrical_type {
                                                ElectricalType::Output |
                                                ElectricalType::PowerOutput |
                                                ElectricalType::Bidirectional |
                                                ElectricalType::OpenCollector |
                                                ElectricalType::TriState |
                                                ElectricalType::Passive => {
                                                    has_driver = true;
                                                    break;
                                                }
                                                _ => {}
                                            }
                                        }
                                    }
                                }
                                
                                if !has_driver {
                                    issues.push(LintIssue {
                                        severity: LintSeverity::Warning,
                                        rule_id: "E002_NO_DRIVER_NET".to_string(),
                                        message: format!("Red '{}' sin emisor de señal (driver)", net_name),
                                        details: format!("El pin de entrada {}.{} está conectado a la red '{}', pero ningún componente de esa red provee señal (Output, Bidirectional o Passive).", des, pin_id, net_name),
                                    });
                                }
                            }
                        }
                    }
                }
                
                ElectricalType::PowerInput => {
                    // Regla 3: Pines de alimentación huerfanos en integrados
                    if is_ic && connected_net.is_none() {
                        issues.push(LintIssue {
                            severity: LintSeverity::Warning,
                            rule_id: "E003_UNCONNECTED_POWER".to_string(),
                            message: format!("Pin de alimentación sin conectar en {}.{}", des, pin_id),
                            details: format!("El pin de alimentación '{}' ({}) del circuito integrado '{}' no tiene ninguna conexión eléctrica.", pin.name, pin_id, des),
                        });
                    }
                }
                _ => {}
            }
        }
    }

    // Regla 2: Cortocircuitos en redes (Output Contention / Shorts)
    for (net_name, net) in &design.nets {
        let mut output_pins = Vec::new();
        
        for ep in &net.endpoints {
            if let Some(comp) = design.components.get(&ep.component_designator) {
                if let Some(pin) = comp.pins.get(&ep.pin_id) {
                    if pin.electrical_type == ElectricalType::Output || pin.electrical_type == ElectricalType::PowerOutput {
                        output_pins.push(format!("{}.{} ({})", ep.component_designator, ep.pin_id, pin.name));
                    }
                }
            }
        }
        
        if output_pins.len() > 1 {
            issues.push(LintIssue {
                severity: LintSeverity::Critical,
                rule_id: "E004_OUTPUT_SHORT".to_string(),
                message: format!("Cortocircuito / Conflicto de salidas en la red '{}'", net_name),
                details: format!("Se detectaron múltiples salidas conectadas directamente a la red '{}': {}. Esto causará conflicto lógico o daño físico por sobrecorriente.", net_name, output_pins.join(", ")),
            });
        }
    }

    issues
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{Component, Pin};

    #[test]
    fn test_lint_floating_input() {
        let mut design = HardwareDesign::new();
        // Componente con pin input flotante
        let comp = Component::new("U1", "SOIC-8")
            .with_pin(Pin::new("1", "IN", ElectricalType::Input));
        design.add_component(comp);

        let issues = run_lint(&design);
        assert!(issues.iter().any(|i| i.rule_id == "E001_FLOATING_INPUT"));
    }

    #[test]
    fn test_lint_output_short() {
        let mut design = HardwareDesign::new();
        
        let u1 = Component::new("U1", "SOIC-8")
            .with_pin(Pin::new("1", "OUT1", ElectricalType::Output));
        let u2 = Component::new("U2", "SOIC-8")
            .with_pin(Pin::new("1", "OUT2", ElectricalType::Output));
            
        design.add_component(u1);
        design.add_component(u2);
        
        // Conectar salidas juntas en la misma red
        design.add_net_endpoint("conflict_net", "U1", "1");
        design.add_net_endpoint("conflict_net", "U2", "1");
        
        let issues = run_lint(&design);
        assert!(issues.iter().any(|i| i.rule_id == "E004_OUTPUT_SHORT"));
    }
}
