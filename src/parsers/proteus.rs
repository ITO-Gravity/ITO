// src/parsers/proteus.rs
//
// Parser nativo de proyectos Proteus (.pdsprj).
//
// Un `.pdsprj` es un contenedor ZIP. El esquemático vive en la entrada `ROOT.DSN`
// (formato ISIS, empieza con el texto "ISIS SCHEMATIC FILE"). Es un archivo
// semi-binario, pero los datos de cada componente colocado se guardan como objetos
// de texto ASCII con "marcadores" que etiquetan el contenido anterior:
//
//   <contenido> <bytes-de-coordenadas> "Default Font" <MARCADOR>
//
// Marcadores relevantes por componente (en orden):
//   COMPONENT ID     -> el designador (R1, U1, D1, ...)
//   COMPONENT VALUE  -> el valor (15k, LED, ...)
//   SUBCKT NAME      -> el nombre de la parte/librería
//   PROPERTIES       -> bloque con tokens {PACKAGE=...} {PRIMTYPE=...} {PRIMITIVE=...}
//
// Esta v0.1 extrae componentes + valor + footprint + tipo. Las conexiones (nets)
// quedan para una segunda etapa (marcadores $MKRNODE / WIRE / $PINBUS).

use std::io::Read;
use std::path::Path;
use anyhow::{Result, anyhow};
use crate::models::{HardwareDesign, Component};

/// Parsea un proyecto Proteus `.pdsprj` y devuelve el diseño de hardware (componentes).
pub fn parse_proteus<P: AsRef<Path>>(path: P) -> Result<HardwareDesign> {
    let file = std::fs::File::open(&path)
        .map_err(|e| anyhow!("No se pudo abrir el archivo Proteus '{}': {}", path.as_ref().display(), e))?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|e| anyhow!("El .pdsprj no es un ZIP válido (¿archivo corrupto?): {}", e))?;

    // El esquemático está en ROOT.DSN.
    let mut dsn_bytes = Vec::new();
    {
        let mut dsn = archive.by_name("ROOT.DSN")
            .map_err(|_| anyhow!("El .pdsprj no contiene 'ROOT.DSN' (esquemático ISIS)"))?;
        dsn.read_to_end(&mut dsn_bytes)
            .map_err(|e| anyhow!("Error leyendo ROOT.DSN: {}", e))?;
    }

    let tokens = tokenize_ascii(&dsn_bytes);
    Ok(extract_components(&tokens))
}

/// Corta el flujo de bytes en tokens de texto ASCII imprimible (separando en cualquier
/// byte no imprimible). Así aislamos las cadenas legibles del framing binario.
fn tokenize_ascii(bytes: &[u8]) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut cur = String::new();
    for &b in bytes {
        if (32..=126).contains(&b) {
            cur.push(b as char);
        } else if !cur.is_empty() {
            tokens.push(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        tokens.push(cur);
    }
    tokens
}

/// Intenta extraer un designador de componente del inicio del token: letras (1-4) seguidas
/// de dígitos (1-4): R1, C12, U3, SW1, LED2. Tolera un sufijo corto de coordenada pegado por
/// el framing binario (p. ej. "U1p" -> "U1"), pero no si el sufijo contiene dígitos.
/// Devuelve el designador normalizado, o None si el token no lo es.
fn extract_designator(tok: &str) -> Option<String> {
    let t = tok.trim();
    let bytes = t.as_bytes();
    let mut i = 0;
    while i < bytes.len() && bytes[i].is_ascii_alphabetic() { i += 1; }
    let letters = i;
    let digits_start = i;
    while i < bytes.len() && bytes[i].is_ascii_digit() { i += 1; }
    let digits = i - digits_start;
    if !((1..=4).contains(&letters) && (1..=4).contains(&digits)) {
        return None;
    }
    // Sufijo restante: aceptar sólo hasta 1 carácter no numérico (byte de coordenada).
    let suffix = &t[i..];
    if suffix.len() > 1 || suffix.chars().any(|c| c.is_ascii_digit()) {
        return None;
    }
    Some(t[..i].to_string())
}

/// ¿El token es "sustancial" (candidato a valor), y no un marcador o coordenada suelta?
fn is_substantial(tok: &str) -> bool {
    let t = tok.trim();
    t.len() >= 2
        && t != "Default Font"
        && !t.starts_with('{')
        && t.chars().any(|c| c.is_ascii_alphanumeric())
}

/// Extrae el valor de un token del estilo `{CLAVE=valor}` (tolera prefijos como `#{CLAVE=..}`).
fn extract_braced(tok: &str, key: &str) -> Option<String> {
    let needle = format!("{{{}=", key); // "{PACKAGE="
    let start = tok.find(&needle)? + needle.len();
    let rest = &tok[start..];
    let end = rest.find('}')?;
    Some(rest[..end].trim().to_string())
}

/// Recorre los tokens y reconstruye los componentes colocados usando el marcador COMPONENT ID
/// como ancla de cada registro.
fn extract_components(tokens: &[String]) -> HardwareDesign {
    let mut design = HardwareDesign::new();
    let n = tokens.len();

    // Posiciones de cada marcador COMPONENT ID (ancla de cada componente colocado).
    let id_positions: Vec<usize> = (0..n).filter(|&i| tokens[i].trim() == "COMPONENT ID").collect();

    for (idx, &i) in id_positions.iter().enumerate() {
        // El registro de este componente termina donde empieza el siguiente COMPONENT ID.
        let record_end = id_positions.get(idx + 1).copied().unwrap_or(n);

        // DESIGNADOR: el marcador etiqueta el contenido ANTERIOR -> escanear hacia atrás
        // el primer token con forma de designador (dentro de una ventana corta).
        let mut designator = None;
        let lo = i.saturating_sub(10);
        for j in (lo..i).rev() {
            if let Some(d) = extract_designator(&tokens[j]) {
                designator = Some(d);
                break;
            }
        }
        let designator = match designator {
            Some(d) => d,
            None => continue, // sin designador reconocible: no es un componente real
        };

        // VALOR: primer token sustancial entre COMPONENT ID y COMPONENT VALUE.
        let mut value = None;
        for k in (i + 1)..record_end {
            if tokens[k].trim() == "COMPONENT VALUE" {
                break;
            }
            if is_substantial(&tokens[k]) {
                value = Some(tokens[k].trim().to_string());
                break;
            }
        }

        // FOOTPRINT y TIPO: del bloque PROPERTIES ({PACKAGE=...} / {PRIMTYPE=...}).
        let mut footprint = None;
        let mut primtype = None;
        for k in i..record_end {
            let t = tokens[k].trim();
            if footprint.is_none() {
                if let Some(v) = extract_braced(t, "PACKAGE") {
                    footprint = Some(v);
                }
            }
            if primtype.is_none() {
                if let Some(v) = extract_braced(t, "PRIMTYPE") {
                    primtype = Some(v);
                }
            }
        }

        // Dedup por designador: la primera instancia gana.
        if design.components.contains_key(&designator) {
            continue;
        }

        let mut comp = Component::new(&designator, footprint.as_deref().unwrap_or(""));
        comp.value = value;
        if let Some(pt) = primtype {
            comp.add_attribute("primtype", &pt);
        }
        design.add_component(comp);
    }

    design
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_designator() {
        assert_eq!(extract_designator("R1"), Some("R1".to_string()));
        assert_eq!(extract_designator("SW12"), Some("SW12".to_string()));
        assert_eq!(extract_designator("U1p"), Some("U1".to_string())); // sufijo de coordenada
        assert_eq!(extract_designator("Default Font"), None);
        assert_eq!(extract_designator("R"), None);
        assert_eq!(extract_designator("123"), None);
        assert_eq!(extract_designator("{PACKAGE=RES180}"), None);
        assert_eq!(extract_designator("R12xy"), None); // sufijo demasiado largo (2 chars)
    }

    #[test]
    fn test_extract_braced() {
        assert_eq!(extract_braced("{PACKAGE=RES180}", "PACKAGE"), Some("RES180".to_string()));
        assert_eq!(extract_braced("#{PACKAGE=MODULE_ESP32C3_SUPERMINI}", "PACKAGE"), Some("MODULE_ESP32C3_SUPERMINI".to_string()));
        assert_eq!(extract_braced("{PRIMTYPE=RESISTOR}", "PRIMTYPE"), Some("RESISTOR".to_string()));
        assert_eq!(extract_braced("nada", "PACKAGE"), None);
    }

    #[test]
    fn test_extract_components_synthetic() {
        // Simula el patrón de tokens de un registro Proteus: <ref> <coords> DefaultFont
        // COMPONENT ID <valor> ... COMPONENT VALUE <parte> ... PROPERTIES {PACKAGE} {PRIMTYPE}
        let toks: Vec<String> = [
            "R1", "P~,", "2", "H", "Default Font", "COMPONENT ID",
            "15k", "(", "5", "Default Font", "COMPONENT VALUE",
            "10WATT1K", "$", "Default Font", "SUBCKT NAME",
            "Default Font", "PROPERTIES", "{PRIMTYPE=RESISTOR}", "{PACKAGE=RES180}",
        ].iter().map(|s| s.to_string()).collect();

        let design = extract_components(&toks);
        assert_eq!(design.components.len(), 1);
        let r1 = design.components.get("R1").expect("R1 debe existir");
        assert_eq!(r1.value.as_deref(), Some("15k"));
        assert_eq!(r1.footprint.as_deref(), Some("RES180"));
        assert_eq!(r1.attributes.get("primtype").map(|s| s.as_str()), Some("RESISTOR"));
    }
}
