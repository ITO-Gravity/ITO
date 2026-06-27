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

    let mut design_exists = false;
    let mut bom_exists = false;
    if let Ok(entries) = std::fs::read_dir(&path) {
        for entry in entries.flatten() {
            if let Some(ext) = entry.path().extension().and_then(|s| s.to_str()) {
                let ext_lower = ext.to_lowercase();
                if ext_lower == "kicad_pcb" || ext_lower == "kicad_sch" || ext_lower == "brd" || ext_lower == "sch" || ext_lower == "json" {
                    design_exists = true;
                }
                if ext_lower == "xlsx" || ext_lower == "xls" || ext_lower == "csv" {
                    bom_exists = true;
                }
            }
        }
    }

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
    
    let new_design = ito::parsers::parse_project_directory(&path)
        .map_err(|e| format!("Error al analizar el diseño local: {}", e))?;

    let cache_dir = path.join(".ito").join("cache");
    let mut old_design_opt = None;
    
    let old_design = if cache_dir.exists() {
        if let Ok(design) = ito::parsers::parse_project_directory(&cache_dir) {
            old_design_opt = Some(design.clone());
            design
        } else {
            ito::models::HardwareDesign::new()
        }
    } else {
        ito::models::HardwareDesign::new()
    };

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
