// src/cas.rs

use std::path::{Path, PathBuf};
use std::fs;
use sha2::{Sha256, Digest};

/// Devuelve la ruta física dentro del CAS donde vive (o viviría) el objeto con este hash.
pub fn object_path(hash: &str, objects_dir: &Path) -> Option<PathBuf> {
    if hash.len() < 4 {
        return None;
    }
    Some(objects_dir.join(&hash[0..2]).join(&hash[2..]))
}

/// Indica si un objeto existe físicamente en el almacén CAS. Se usa para verificar la integridad
/// ANTES de una restauración, de modo que nunca se modifique el working dir si falta un objeto.
pub fn object_exists(hash: &str, objects_dir: &Path) -> bool {
    object_path(hash, objects_dir).map(|p| p.exists()).unwrap_or(false)
}

/// Calcula el hash SHA-256 de un archivo físico en disco
pub fn calculate_file_hash(path: &Path) -> Result<String, String> {
    let bytes = fs::read(path)
        .map_err(|e| format!("Error al leer archivo para hash {}: {}", path.display(), e))?;
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    Ok(format!("{:x}", hasher.finalize()))
}

/// Guarda un archivo en el almacén direccionable por contenido (.ito/objects/)
/// Retorna el hash SHA-256 calculado.
pub fn store_file(file_path: &Path, objects_dir: &Path) -> Result<String, String> {
    if !file_path.is_file() {
        return Err(format!("La ruta '{}' no es un archivo válido.", file_path.display()));
    }

    let hash = calculate_file_hash(file_path)?;
    if hash.len() < 4 {
        return Err("Hash calculado inválido (demasiado corto).".to_string());
    }

    let prefix = &hash[0..2];
    let suffix = &hash[2..];
    let dest_dir = objects_dir.join(prefix);
    let dest_file = dest_dir.join(suffix);

    if !dest_file.exists() {
        fs::create_dir_all(&dest_dir)
            .map_err(|e| format!("Error al crear subdirectorio CAS {}: {}", dest_dir.display(), e))?;
        fs::copy(file_path, &dest_file)
            .map_err(|e| format!("Error al copiar archivo al CAS ({} -> {}): {}", file_path.display(), dest_file.display(), e))?;
    }

    Ok(hash)
}

/// Restaura un archivo desde el almacén de objetos CAS (.ito/objects/) hacia su destino original
pub fn restore_file(hash: &str, dest_path: &Path, objects_dir: &Path) -> Result<(), String> {
    if hash.len() < 4 {
        return Err(format!("Hash CAS inválido: {}", hash));
    }

    let prefix = &hash[0..2];
    let suffix = &hash[2..];
    let src_file = objects_dir.join(prefix).join(suffix);

    if !src_file.exists() {
        return Err(format!("No se encontró el objeto con hash '{}' en el almacén CAS.", hash));
    }

    if let Some(parent) = dest_path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Error al crear directorios para restaurar {}: {}", parent.display(), e))?;
    }

    // Restauración atómica: copiar a un temporal en el mismo directorio y renombrar sobre el destino.
    // Evita dejar un archivo a medio escribir si el proceso se interrumpe durante la copia.
    let file_name = dest_path.file_name().and_then(|n| n.to_str()).unwrap_or("ito");
    let tmp_path = dest_path.with_file_name(format!(".{}.itotmp-{}", file_name, uuid::Uuid::new_v4()));

    fs::copy(&src_file, &tmp_path)
        .map_err(|e| format!("Error al restaurar archivo desde CAS ({} -> {}): {}", src_file.display(), tmp_path.display(), e))?;

    if let Err(e) = fs::rename(&tmp_path, dest_path) {
        let _ = fs::remove_file(&tmp_path);
        return Err(format!("Error al confirmar restauración de {}: {}", dest_path.display(), e));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cas_store_and_restore() {
        let unique_id = uuid::Uuid::new_v4().to_string();
        let temp_dir = std::env::temp_dir().join(format!("ito-cas-test-{}", unique_id));
        let objects_dir = temp_dir.join("objects");
        fs::create_dir_all(&objects_dir).unwrap();

        let test_file = temp_dir.join("test.txt");
        let content = "Contenido de prueba para el almacenamiento CAS de ITO.";
        fs::write(&test_file, content).unwrap();

        let hash = store_file(&test_file, &objects_dir).unwrap();
        assert!(!hash.is_empty());

        let prefix = &hash[0..2];
        let suffix = &hash[2..];
        let stored_path = objects_dir.join(prefix).join(suffix);
        assert!(stored_path.exists());

        let restored_file = temp_dir.join("restored.txt");
        restore_file(&hash, &restored_file, &objects_dir).unwrap();
        assert!(restored_file.exists());

        let restored_content = fs::read_to_string(&restored_file).unwrap();
        assert_eq!(restored_content, content);

        fs::remove_dir_all(&temp_dir).ok();
    }
}