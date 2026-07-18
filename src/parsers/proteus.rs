// src/parsers/proteus.rs
//
// Parser nativo de proyectos Proteus (.pdsprj).
//
// Un `.pdsprj` es un contenedor ZIP. El esquemático vive en la entrada `ROOT.DSN`
// (formato ISIS, empieza con el texto "ISIS SCHEMATIC FILE"). Es un archivo binario,
// pero su estructura de strings es decodificable:
//
//   * Los MARCADORES de campo son cadenas terminadas en NUL:  "COMPONENT ID\0",
//     "COMPONENT VALUE\0", "SUBCKT NAME\0", "Default Font\0", "OBJECT DATA\0".
//   * Los CONTENIDOS (referencia, valor, nombre de parte, propiedades) son cadenas
//     con prefijo de longitud:  0xFF <len:u8> <len bytes>.
//
// Estructura de un componente colocado (dentro de "OBJECT DATA"):
//   FF <len> <REFERENCIA>            (p. ej. "R1", "U1")   <- designador
//   <coordenadas binarias>
//   ... "Default Font\0" "COMPONENT ID\0"  FF <len> <VALOR>   (p. ej. "15k")
//   ... "Default Font\0" "COMPONENT VALUE\0" FF <len> <PARTE>
//   ... "SUBCKT NAME\0" ...  {PACKAGE=...} {PRIMTYPE=...}      (footprint / tipo)
//
// Leer los contenidos por su prefijo de longitud evita el artefacto de "byte pegado"
// (p. ej. "ESP32C3_SUPERMINI_SMDp" -> "ESP32C3_SUPERMINI_SMD").
//
// Esta versión extrae componentes + valor + footprint + tipo. Las conexiones (nets)
// son geométricas en ISIS (cables por coordenadas) y quedan para una etapa posterior.

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

    Ok(extract_components(&dsn_bytes))
}

/// Lee una cadena con prefijo de longitud (`0xFF <len> <len bytes>`) en `pos`.
/// Devuelve la cadena y la posición siguiente, o None si no hay un prefijo válido allí.
fn read_len_prefixed(bytes: &[u8], pos: usize) -> Option<(String, usize)> {
    if pos + 1 >= bytes.len() || bytes[pos] != 0xFF {
        return None;
    }
    let len = bytes[pos + 1] as usize;
    let start = pos + 2;
    let end = start + len;
    if end > bytes.len() {
        return None;
    }
    Some((String::from_utf8_lossy(&bytes[start..end]).to_string(), end))
}

/// Devuelve todos los offsets donde aparece `needle` dentro de `haystack`.
fn find_all(haystack: &[u8], needle: &[u8]) -> Vec<usize> {
    let mut res = Vec::new();
    if needle.is_empty() || haystack.len() < needle.len() {
        return res;
    }
    let mut i = 0;
    while i + needle.len() <= haystack.len() {
        if &haystack[i..i + needle.len()] == needle {
            res.push(i);
            i += needle.len();
        } else {
            i += 1;
        }
    }
    res
}

/// Intenta extraer un designador de componente del inicio de la cadena: letras (1-4)
/// seguidas de dígitos (1-4): R1, C12, U3, SW1, LED2. Tolera un sufijo corto de coordenada
/// (por robustez), pero con las lecturas por prefijo de longitud ya no debería hacer falta.
fn extract_designator(s: &str) -> Option<String> {
    let t = s.trim();
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
    let suffix = &t[i..];
    if suffix.len() > 1 || suffix.chars().any(|c| c.is_ascii_digit()) {
        return None;
    }
    Some(t[..i].to_string())
}

/// Extrae el valor de un token del estilo `{CLAVE=valor}` dentro de una cadena mayor.
fn extract_braced(text: &str, key: &str) -> Option<String> {
    let needle = format!("{{{}=", key); // "{PACKAGE="
    let start = text.find(&needle)? + needle.len();
    let rest = &text[start..];
    let end = rest.find('}')?;
    Some(rest[..end].trim().to_string())
}

/// Recorre el binario y reconstruye los componentes colocados usando el marcador
/// "COMPONENT ID" (terminado en NUL) como ancla de cada registro.
fn extract_components(bytes: &[u8]) -> HardwareDesign {
    let mut design = HardwareDesign::new();
    let marker = b"COMPONENT ID\0";
    let id_offsets = find_all(bytes, marker);

    for (idx, &off) in id_offsets.iter().enumerate() {
        // El registro de este componente termina donde empieza el siguiente COMPONENT ID.
        let record_end = id_offsets.get(idx + 1).copied().unwrap_or(bytes.len());

        // DESIGNADOR: la referencia es la cadena con prefijo de longitud (FF <len> ..) más
        // cercana ANTES del marcador que forme un designador válido (R1, U1, ...).
        let mut designator = None;
        let lo = off.saturating_sub(80);
        let mut q = off;
        while q > lo {
            q -= 1;
            if bytes[q] == 0xFF {
                if let Some((s, _)) = read_len_prefixed(bytes, q) {
                    if let Some(d) = extract_designator(&s) {
                        designator = Some(d);
                        break;
                    }
                }
            }
        }
        let designator = match designator {
            Some(d) => d,
            None => continue,
        };

        // VALOR: primera cadena con prefijo de longitud DESPUÉS del marcador COMPONENT ID.
        let after = off + marker.len();
        let scan_end = (after + 12).min(bytes.len());
        let mut value = None;
        let mut p = after;
        while p < scan_end {
            if bytes[p] == 0xFF {
                if let Some((s, _)) = read_len_prefixed(bytes, p) {
                    value = Some(s);
                }
                break;
            }
            p += 1;
        }

        // FOOTPRINT y TIPO: del bloque PROPERTIES del registro ({PACKAGE=..} / {PRIMTYPE=..}).
        let record = &bytes[off..record_end.min(bytes.len())];
        let record_str = String::from_utf8_lossy(record);
        let footprint = extract_braced(&record_str, "PACKAGE");
        let primtype = extract_braced(&record_str, "PRIMTYPE");

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
    fn test_read_len_prefixed() {
        let buf = [0xFF, 0x03, b'1', b'5', b'k', 0x99];
        assert_eq!(read_len_prefixed(&buf, 0), Some(("15k".to_string(), 5)));
        assert_eq!(read_len_prefixed(&buf, 5), None); // no empieza con 0xFF
    }

    #[test]
    fn test_extract_designator() {
        assert_eq!(extract_designator("R1"), Some("R1".to_string()));
        assert_eq!(extract_designator("SW12"), Some("SW12".to_string()));
        assert_eq!(extract_designator("U1"), Some("U1".to_string()));
        assert_eq!(extract_designator("Default Font"), None);
        assert_eq!(extract_designator("R"), None);
        assert_eq!(extract_designator("{PACKAGE=RES180}"), None);
    }

    #[test]
    fn test_extract_braced() {
        assert_eq!(extract_braced("...{PACKAGE=RES180}...", "PACKAGE"), Some("RES180".to_string()));
        assert_eq!(extract_braced("x{PRIMTYPE=RESISTOR}y", "PRIMTYPE"), Some("RESISTOR".to_string()));
        assert_eq!(extract_braced("nada", "PACKAGE"), None);
    }

    #[test]
    fn test_extract_components_binary() {
        // Registro sintético: referencia R1 (FF 02), marcador COMPONENT ID, valor 15k (FF 03),
        // y un bloque de propiedades con footprint/tipo. Emula el layout binario real.
        let mut buf: Vec<u8> = Vec::new();
        buf.extend_from_slice(&[0xFF, 0x02]);
        buf.extend_from_slice(b"R1"); // referencia
        buf.extend_from_slice(&[0x00, 0x00, 0x12]); // coords binarias de relleno
        buf.extend_from_slice(b"COMPONENT ID\0");
        buf.extend_from_slice(&[0x00, 0x00, 0xFF, 0x03]);
        buf.extend_from_slice(b"15k"); // valor
        buf.extend_from_slice(b"---{PRIMTYPE=RESISTOR}--{PACKAGE=RES180}---");

        let design = extract_components(&buf);
        assert_eq!(design.components.len(), 1);
        let r1 = design.components.get("R1").expect("R1 debe existir");
        assert_eq!(r1.value.as_deref(), Some("15k"));
        assert_eq!(r1.footprint.as_deref(), Some("RES180"));
        assert_eq!(r1.attributes.get("primtype").map(|s| s.as_str()), Some("RESISTOR"));
    }
}
