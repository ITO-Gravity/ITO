use std::path::PathBuf;

#[derive(serde::Serialize)]
pub struct ProjectStatus {
    project_id: String,
    remote_url: String,
    design_exists: bool,
    bom_exists: bool,
    history: Vec<ito::CommitEntry>,
}

#[derive(serde::Serialize)]
pub struct PushResult {
    success: bool,
    message: String,
    commit: Option<ito::CommitEntry>,
}

#[tauri::command]
fn select_folder(app: tauri::AppHandle) -> Option<String> {
    let (tx, rx) = std::sync::mpsc::channel();
    app.run_on_main_thread(move || {
        let path = rfd::FileDialog::new()
            .set_title("Seleccionar Carpeta del Proyecto de Hardware")
            .pick_folder()
            .map(|p| p.to_string_lossy().to_string());
        let _ = tx.send(path);
    }).ok();
    rx.recv().unwrap_or(None)
}

#[tauri::command]
fn load_project_status(dir: String) -> Result<ProjectStatus, String> {
    let path = PathBuf::from(&dir);
    
    let config_path = path.join(".ito").join("config.toml");
    let (project_id, remote_url) = if config_path.exists() {
        let content = std::fs::read_to_string(&config_path)
            .map_err(|e| format!("Error al leer configuración: {}", e))?;
        let config: ito::Config = toml::from_str(&content)
            .map_err(|e| format!("Error al parsear configuración: {}", e))?;
        (config.project_id, config.remote_url)
    } else {
        ("No inicializado".to_string(), "".to_string())
    };

    let design_exists = path.join("design.json").exists();
    let bom_exists = path.join("bom.csv").exists();

    // Cargar historial
    let history_path = path.join(".ito").join("history.toml");
    let history = if history_path.exists() {
        let content = std::fs::read_to_string(&history_path).unwrap_or_default();
        let parsed: ito::History = toml::from_str(&content).unwrap_or_default();
        parsed.commits
    } else {
        Vec::new()
    };

    Ok(ProjectStatus {
        project_id,
        remote_url,
        design_exists,
        bom_exists,
        history,
    })
}

#[derive(serde::Serialize)]
pub struct DesignsPayload {
    old_design: Option<ito::models::HardwareDesign>,
    new_design: ito::models::HardwareDesign,
    diff: ito::diff::DesignDiff,
}

#[tauri::command]
fn calculate_diff(dir: String) -> Result<DesignsPayload, String> {
    let path = PathBuf::from(&dir);
    let new_cad = path.join("design.json");
    let new_bom = path.join("bom.csv");

    if !new_cad.exists() {
        return Err("No se encontró design.json en la carpeta seleccionada.".to_string());
    }

    let cache_dir = path.join(".ito").join("cache");
    let old_cad = cache_dir.join("design.old.json");
    let old_bom = cache_dir.join("bom.old.csv");

    let mut old_design_opt = None;
    let old_design = if old_cad.exists() {
        let mut design = ito::parsers::parse_cad_json(&old_cad)
            .map_err(|e| format!("Error al parsear design.old.json: {}", e))?;
        if old_bom.exists() {
            let bom = ito::parsers::parse_bom_csv(&old_bom)
                .map_err(|e| format!("Error al parsear bom.old.csv: {}", e))?;
            design.merge_bom(bom);
        }
        old_design_opt = Some(design.clone());
        design
    } else {
        ito::models::HardwareDesign::new()
    };

    let mut new_design = ito::parsers::parse_cad_json(&new_cad)
        .map_err(|e| format!("Error al parsear design.json: {}", e))?;
    if new_bom.exists() {
        let bom = ito::parsers::parse_bom_csv(&new_bom)
            .map_err(|e| format!("Error al parsear bom.csv: {}", e))?;
        new_design.merge_bom(bom);
    }

    let diff_result = ito::diff::diff_designs(&old_design, &new_design);
    Ok(DesignsPayload {
        old_design: old_design_opt,
        new_design,
        diff: diff_result,
    })
}

#[tauri::command]
async fn push_project(dir: String, message: Option<String>) -> Result<PushResult, String> {
    let path = PathBuf::from(&dir);
    match ito::run_push(path, message).await {
        Ok((commit, msg)) => {
            Ok(PushResult {
                success: true,
                message: msg,
                commit: Some(commit),
            })
        }
        Err(err_msg) => {
            Ok(PushResult {
                success: false,
                message: err_msg,
                commit: None,
            })
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            select_folder,
            load_project_status,
            calculate_diff,
            push_project
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
