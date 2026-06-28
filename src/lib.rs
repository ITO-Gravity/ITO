pub mod models;
pub mod parsers;
pub mod diff;
pub mod linter;

use sha2::{Sha256, Digest};
use std::io::Write;

#[derive(Debug, serde::Serialize, serde::Deserialize, Clone)]
pub struct Config {
    pub project_id: String,
    pub remote_url: String,
}

#[derive(Debug, serde::Serialize, serde::Deserialize, Clone)]
pub struct CommitEntry {
    pub hash: String,
    pub parent_hash: String,
    pub message: String,
    pub timestamp: String,
    pub zip_path: String,
    pub synced: bool,
}

#[derive(Debug, Default, serde::Serialize, serde::Deserialize, Clone)]
pub struct History {
    pub commits: Vec<CommitEntry>,
}

pub async fn run_push(project_dir: std::path::PathBuf, message: Option<String>) -> Result<(CommitEntry, String), String> {
    let config_path = project_dir.join(".ito").join("config.toml");
    if !config_path.exists() {
        return Err("No se encontró la configuración de Ito. ¿Corriste 'ito init' primero?".to_string());
    }

    // 1. Leer configuración TOML
    let config_str = std::fs::read_to_string(&config_path)
        .map_err(|e| format!("Error al leer configuración: {}", e))?;
    let config: Config = toml::from_str(&config_str)
        .map_err(|e| format!("Error al parsear configuración: {}", e))?;

    // 2. Parsear el proyecto de hardware (detecta y unifica cualquier formato nativo)
    let design = parsers::parse_project_directory(&project_dir)
        .map_err(|e| format!("Error al analizar el directorio de hardware: {}", e))?;

    // 3. Serializar diseño a bytes normalizados para hashing y empaquetado
    let design_bytes = serde_json::to_vec_pretty(&design)
        .map_err(|e| format!("Error al serializar diseño normalizado: {}", e))?;

    let mut wtr = csv::Writer::from_writer(Vec::new());
    wtr.write_record(&["Designator", "MPN", "Manufacturer", "Value", "Footprint"]).ok();
    for (des, comp) in &design.components {
        wtr.write_record(&[
            des.as_str(),
            comp.mpn.as_deref().unwrap_or(""),
            comp.manufacturer.as_deref().unwrap_or(""),
            comp.value.as_deref().unwrap_or(""),
            comp.footprint.as_deref().unwrap_or(""),
        ]).ok();
    }
    let bom_bytes = wtr.into_inner().unwrap_or_default();

    let mut hasher = Sha256::new();
    hasher.update(&design_bytes);
    hasher.update(&bom_bytes);
    
    let hash_result = hasher.finalize();
    let hash_str = format!("{:x}", hash_result);

    // 4. Cargar historial local
    let history_path = project_dir.join(".ito").join("history.toml");
    let mut history = if history_path.exists() {
        let content = std::fs::read_to_string(&history_path)
            .map_err(|e| format!("Error al leer historial: {}", e))?;
        toml::from_str(&content).unwrap_or_default()
    } else {
        History::default()
    };

    let parent_hash = history
        .commits
        .last()
        .map(|c| c.hash.clone())
        .unwrap_or_else(|| "0000000000000000000000000000000000000000000000000000000000000000".to_string());

    // Si el hash coincide con el último commit, no hay cambios
    if hash_str == parent_hash {
        return Err("No hay cambios pendientes de hardware para respaldar ni sincronizar.".to_string());
    }

    // 5. Crear la carpeta de respaldos y el ZIP
    let backups_dir = project_dir.join(".ito").join("backups");
    std::fs::create_dir_all(&backups_dir)
        .map_err(|e| format!("Error al crear carpeta backups: {}", e))?;
    let zip_path = backups_dir.join(format!("{}.zip", hash_str));
    let zip_file = std::fs::File::create(&zip_path)
        .map_err(|e| format!("Error al crear archivo zip: {}", e))?;
    let mut zip = zip::ZipWriter::new(zip_file);

    let options = zip::write::FileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);

    // Adjuntar design.json al ZIP
    zip.start_file("design.json", options)
        .map_err(|e| format!("Error al añadir design.json al zip: {}", e))?;
    zip.write_all(&design_bytes)
        .map_err(|e| format!("Error al escribir design.json al zip: {}", e))?;

    // Adjuntar bom.csv al ZIP (si contiene componentes)
    if !design.components.is_empty() {
        zip.start_file("bom.csv", options)
            .map_err(|e| format!("Error al añadir bom.csv al zip: {}", e))?;
        zip.write_all(&bom_bytes)
            .map_err(|e| format!("Error al escribir bom.csv al zip: {}", e))?;
    }
    zip.finish()
        .map_err(|e| format!("Error al finalizar zip: {}", e))?;

    // 6. Registrar en el historial local
    let timestamp = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let commit_msg = message.clone().unwrap_or_else(|| "Sincronización de hardware".to_string());
    let relative_zip_path = format!(".ito/backups/{}.zip", hash_str);

    let mut commit_entry = CommitEntry {
        hash: hash_str.clone(),
        parent_hash: parent_hash.clone(),
        message: commit_msg.clone(),
        timestamp,
        zip_path: relative_zip_path,
        synced: false,
    };

    // 7. Intentar la sincronización con Alexandria-HQ
    let mut form = reqwest::multipart::Form::new()
        .text("project_id", config.project_id.clone())
        .text("domain", "hardware")
        .text("version_hash", hash_str.clone())
        .text("parent_hash", parent_hash.clone())
        .text("message", commit_msg.clone());

    let design_part = reqwest::multipart::Part::bytes(design_bytes.clone())
        .file_name("design.json")
        .mime_str("application/json")
        .map_err(|e| format!("Error al crear parte design_json: {}", e))?;
    form = form.part("design_json", design_part);

    if !design.components.is_empty() {
        let bom_part = reqwest::multipart::Part::bytes(bom_bytes.clone())
            .file_name("bom.csv")
            .mime_str("text/csv")
            .map_err(|e| format!("Error al crear parte bom_csv: {}", e))?;
        form = form.part("bom_csv", bom_part);
    }

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| format!("Error al construir cliente HTTP: {}", e))?;
    
    let send_res = client
        .post(&config.remote_url)
        .multipart(form)
        .send()
        .await;

    let (sync_success, info_msg) = match send_res {
        Ok(response) => {
            if response.status().is_success() {
                (true, format!("¡Archivos del proyecto sincronizados con éxito en Alexandria-HQ! [Proyecto: {}]", config.project_id))
            } else {
                (false, format!("Backup local generado con éxito en .ito/backups/. Sincronización fallida (HTTP {}).", response.status()))
            }
        }
        Err(_) => {
            (false, "Backup local generado con éxito en .ito/backups/. Sincronización con Alexandria-HQ pendiente (Servidor no disponible)".to_string())
        }
    };

    commit_entry.synced = sync_success;
    history.commits.push(commit_entry.clone());
    let history_str = toml::to_string_pretty(&history)
        .map_err(|e| format!("Error al serializar historial: {}", e))?;
    std::fs::write(&history_path, history_str)
        .map_err(|e| format!("Error al escribir historial: {}", e))?;

    // 8. Actualizar la caché local para 'ito diff'
    let cache_dir = project_dir.join(".ito").join("cache");
    std::fs::create_dir_all(&cache_dir).ok();
    std::fs::write(cache_dir.join("design.json"), &design_bytes).ok();
    if !design.components.is_empty() {
        std::fs::write(cache_dir.join("bom.csv"), &bom_bytes).ok();
    } else {
        let cached_old_bom = cache_dir.join("bom.csv");
        if cached_old_bom.exists() {
            std::fs::remove_file(cached_old_bom).ok();
        }
    }

    Ok((commit_entry, info_msg))
}
