pub mod models;
pub mod parsers;
pub mod diff;
pub mod linter;
pub mod engines;
pub mod ignore;
pub mod cas;

use sha2::{Sha256, Digest};

#[derive(Debug, serde::Serialize, serde::Deserialize, Clone)]
pub struct Config {
    pub project_id: String,
    pub remote_url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
}

#[derive(Debug, serde::Serialize, serde::Deserialize, Clone)]
pub struct DiffSummary {
    pub added_components: usize,
    pub deleted_components: usize,
    pub modified_components: usize,
    pub added_nets: usize,
    pub deleted_nets: usize,
    pub modified_nets: usize,
}

#[derive(Debug, serde::Serialize, serde::Deserialize, Clone)]
pub struct CommitEntry {
    pub hash: String,
    pub parent_hash: String,
    pub message: String,
    pub timestamp: String,
    pub zip_path: String,
    pub synced: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diff_summary: Option<DiffSummary>,
    #[serde(default, skip_serializing_if = "std::collections::HashMap::is_empty")]
    pub modules: std::collections::HashMap<String, engines::CommitPayload>,
}

#[derive(Debug, Default, serde::Serialize, serde::Deserialize, Clone)]
pub struct History {
    pub commits: Vec<CommitEntry>,
}

pub fn run_commit(project_dir: std::path::PathBuf, message: Option<String>) -> Result<CommitEntry, String> {
    let ito_dir = project_dir.join(".ito");
    if !ito_dir.exists() {
        return Err("No se encontró la carpeta oculta .ito. ¿Inicializaste el proyecto con 'ito new' o 'ito init'?".to_string());
    }

    // 1. Cargar configuración de ito.json para obtener los módulos activos
    let ito_json_path = project_dir.join("ito.json");
    let mut config: Option<models::ItoProjectConfig> = None;
    if ito_json_path.exists() {
        if let Ok(content) = std::fs::read_to_string(&ito_json_path) {
            config = serde_json::from_str(&content).ok();
        }
    }

    let registry = engines::EngineRegistry::new();
    let mut active_modules: Vec<(String, std::path::PathBuf, String)> = Vec::new();
    if let Some(ref cfg) = config {
        let has_links = cfg.links.as_ref().map(|l| !l.is_empty()).unwrap_or(false);
        if has_links {
            let modules_list = vec![
                ("firmware", cfg.modules.firmware),
                ("electronics", cfg.modules.electronics),
                ("mechanical", cfg.modules.mechanical),
                ("documentation", cfg.modules.documentation),
                ("manufacturing", cfg.modules.manufacturing),
            ];

            for (module_name, is_active) in modules_list {
                if is_active {
                    let mut resolved = false;
                    if let Some(ref links) = cfg.links {
                        if let Some(link) = links.get(module_name) {
                            active_modules.push((module_name.to_string(), std::path::PathBuf::from(&link.path), link.engine.clone()));
                            resolved = true;
                        }
                    }
                    if !resolved {
                        let local_path = project_dir.join(module_name);
                        if local_path.exists() && local_path.is_dir() {
                            let default_engine = if module_name == "electronics" { "semantic-cad".to_string() } else { "file-hash".to_string() };
                            active_modules.push((module_name.to_string(), local_path, default_engine));
                        }
                    }
                }
            }
        }
    }

    // Si no hay módulos vinculados, usar el fallback tradicional (root del proyecto con semantic-cad)
    if active_modules.is_empty() {
        active_modules.push((
            "electronics".to_string(),
            project_dir.clone(),
            "semantic-cad".to_string()
        ));
    }

    // 2. Generar el hash de estado unificado combinando los metadatos de todos los módulos
    let mut state_hasher = Sha256::new();
    if let Some(ref msg) = message {
        state_hasher.update(msg.as_bytes());
    }

    for (key, module_path, engine_name) in &active_modules {
        state_hasher.update(key.as_bytes());
        state_hasher.update(engine_name.as_bytes());
        
        let m_cache_dir = project_dir.join(".ito").join("cache").join(key);
        let engine = registry.get_engine(engine_name).unwrap_or_else(|| registry.get_engine("file-hash").unwrap());
        
        match engine.status(module_path, &m_cache_dir) {
            Ok(engines::ModuleStatus::Modified { summary, details }) => {
                state_hasher.update(summary.as_bytes());
                for d in details {
                    state_hasher.update(d.as_bytes());
                }
            }
            Ok(engines::ModuleStatus::Unchanged) => {
                state_hasher.update(b"unchanged");
            }
            _ => {
                state_hasher.update(b"error");
            }
        }
    }

    let hash_result = state_hasher.finalize();
    let hash_str = format!("{:x}", hash_result);

    // 3. Cargar historial local
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

    if hash_str == parent_hash {
        return Err("No hay cambios pendientes en ningún módulo para confirmar.".to_string());
    }

    // 4. Ejecutar commits en cada motor activo
    let mut modules_payload = std::collections::HashMap::new();
    let mut diff_summary = None;

    for (key, module_path, engine_name) in active_modules {
        let engine = registry.get_engine(&engine_name).unwrap_or_else(|| registry.get_engine("file-hash").unwrap());
        let m_backup_dir = project_dir.join(".ito").join("backups").join(&hash_str).join(&key);
        let m_cache_dir = project_dir.join(".ito").join("cache").join(&key);
        
        // Calcular diff_summary antes de actualizar la caché si es electrónica
        let diff_summary_val = if key == "electronics" {
            let old_design = parsers::parse_project_directory(&m_cache_dir).unwrap_or_else(|_| models::HardwareDesign::new());
            let new_design = parsers::parse_project_directory(&module_path).unwrap_or_else(|_| models::HardwareDesign::new());
            let diff_result = diff::diff_designs(&old_design, &new_design);
            Some(DiffSummary {
                added_components: diff_result.components.added.len(),
                deleted_components: diff_result.components.deleted.len(),
                modified_components: diff_result.components.modified.len(),
                added_nets: diff_result.nets.added.len(),
                deleted_nets: diff_result.nets.deleted.len(),
                modified_nets: diff_result.nets.modified.len(),
            })
        } else {
            None
        };

        let payload = engine.commit(&module_path, &m_backup_dir, &m_cache_dir)?;

        if key == "electronics" {
            diff_summary = diff_summary_val;
        }
        
        modules_payload.insert(key, payload);
    }

    // 5. Guardar commit en el historial
    let timestamp = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let commit_msg = message.unwrap_or_else(|| "Respaldo local del proyecto".to_string());

    let commit_entry = CommitEntry {
        hash: hash_str.clone(),
        parent_hash: parent_hash.clone(),
        message: commit_msg,
        timestamp,
        zip_path: format!(".ito/backups/{}", hash_str),
        synced: true,
        diff_summary,
        modules: modules_payload,
    };

    history.commits.push(commit_entry.clone());
    let history_str = toml::to_string_pretty(&history)
        .map_err(|e| format!("Error al serializar historial: {}", e))?;
    std::fs::write(&history_path, history_str)
        .map_err(|e| format!("Error al escribir historial: {}", e))?;

    Ok(commit_entry)
}

pub fn run_restore(project_dir: std::path::PathBuf, target_hash: &str) -> Result<Vec<String>, String> {
    let history_path = project_dir.join(".ito").join("history.toml");
    if !history_path.exists() {
        return Err("No se encontró el historial del proyecto. ¿Ejecutaste 'ito commit' primero?".to_string());
    }

    // 1. Cargar historial
    let content = std::fs::read_to_string(&history_path)
        .map_err(|e| format!("Error al leer historial: {}", e))?;
    let history: History = toml::from_str(&content)
        .map_err(|e| format!("Error al parsear historial: {}", e))?;

    // 2. Buscar el commit por prefijo
    let matched_commit = history.commits.iter().find(|c| c.hash.starts_with(target_hash))
        .ok_or_else(|| format!("No se encontró ninguna versión con el prefijo de hash '{}'.", target_hash))?;

    let registry = engines::EngineRegistry::new();
    let mut restored_modules = Vec::new();

    // 3. Restauración modular transaccional
    if !matched_commit.modules.is_empty() {
        let ito_json_path = project_dir.join("ito.json");
        let mut links = std::collections::HashMap::new();
        if ito_json_path.exists() {
            if let Ok(c) = std::fs::read_to_string(&ito_json_path) {
                if let Ok(config) = serde_json::from_str::<models::ItoProjectConfig>(&c) {
                    links = config.links.unwrap_or_default();
                }
            }
        }

        for (key, payload) in &matched_commit.modules {
            let path = if let Some(link) = links.get(key) {
                std::path::PathBuf::from(&link.path)
            } else {
                if key == "electronics" {
                    project_dir.clone()
                } else {
                    continue;
                }
            };

            let engine = registry.get_engine(&payload.engine_name)
                .unwrap_or_else(|| registry.get_engine("file-hash").unwrap());

            let m_backup_dir = project_dir.join(".ito").join("backups").join(&matched_commit.hash).join(key);
            engine.restore(&path, &m_backup_dir, payload)?;
            restored_modules.push(key.clone());
        }
    } else {
        // Fallback V1
        let engine = registry.get_engine("semantic-cad").unwrap();
        let m_backup_dir = project_dir.join(".ito").join("backups");
        
        let mut metadata = std::collections::HashMap::new();
        metadata.insert("zip_file".to_string(), format!("{}.zip", matched_commit.hash));
        
        let payload = engines::CommitPayload {
            engine_name: "semantic-cad".to_string(),
            changes_detected: true,
            details: Vec::new(),
            metadata,
        };
        
        engine.restore(&project_dir, &m_backup_dir, &payload)?;
        restored_modules.push("electronics".to_string());
    }

    Ok(restored_modules)
}

pub fn run_new(cwd: std::path::PathBuf, project_name: &str) -> Result<(std::path::PathBuf, String), String> {
    let project_dir = cwd.join(project_name);
    
    // 1. Validar que el proyecto no exista
    if project_dir.exists() {
        return Err(format!("Error: El directorio '{}' ya existe.", project_dir.display()));
    }
    
    // 2. Crear las carpetas de la estructura recursivamente
    let dirs_to_create = [
        "",
        "firmware",
        "electronics",
        "electronics/pcb",
        "electronics/schematics",
        "electronics/libraries",
        "mechanical",
        "mechanical/cad",
        "mechanical/drawings",
        "documentation",
        "manufacturing",
        "assets",
        "scripts",
        "tests",
        ".ito",
        ".ito/backups",
        ".ito/history",
        ".ito/cache",
        ".ito/objects",
        ".ito/logs",
    ];

    for sub_dir in &dirs_to_create {
        if sub_dir.is_empty() {
            continue;
        }
        let path = project_dir.join(sub_dir);
        std::fs::create_dir_all(&path)
            .map_err(|e| format!("Error al crear el directorio '{}': {}", path.display(), e))?;

        // Agregar archivo .gitkeep en las subcarpetas del proyecto (excepto .ito y sus subcarpetas)
        if !sub_dir.starts_with(".ito") {
            let keep_path = path.join(".gitkeep");
            if !keep_path.exists() {
                let _ = std::fs::write(&keep_path, "# Mantenido vacío por ITO\n");
            }
        }
    }

    // 3. Generar archivo ito.json
    let project_uuid = uuid::Uuid::new_v4().to_string();
    let created_at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let created_by = std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "unknown".to_string());

    let config = models::ItoProjectConfig {
        format_version: "1.0".to_string(),
        project_name: project_name.to_string(),
        project_uuid: project_uuid.clone(),
        created_at,
        created_by,
        modules: models::ItoProjectModules {
            firmware: true,
            electronics: true,
            mechanical: true,
            documentation: true,
            manufacturing: true,
        },
        current_revision: "REV-0001".to_string(),
        license: "MIT".to_string(),
        version: "0.1.0".to_string(),
        links: None,
    };

    let ito_json_path = project_dir.join("ito.json");
    let json_content = serde_json::to_string_pretty(&config)
        .map_err(|e| format!("Error al serializar ito.json: {}", e))?;
    std::fs::write(&ito_json_path, json_content)
        .map_err(|e| format!("Error al escribir ito.json: {}", e))?;

    // 4. Generar archivo README.md
    let readme_path = project_dir.join("README.md");
    let readme_content = format!(
        "# {}\n\nProyecto multidisciplinar de ingeniería inicializado con ITO.\n\n\
         ## Módulos del proyecto\n\
         - **Firmware**: Código fuente del firmware.\n\
         - **Electronics**: Diseño electrónico, esquemas y PCBs.\n\
         - **Mechanical**: Planos mecánicos y CAD.\n\
         - **Documentation**: Manuales, datasheets y guías.\n\
         - **Manufacturing**: Archivos de fabricación (Gerbers, BOM, DXF).\n",
        project_name
    );
    std::fs::write(&readme_path, readme_content)
        .map_err(|e| format!("Error al escribir README.md: {}", e))?;

    // 5. Generar archivo LICENSE (MIT por defecto)
    let license_path = project_dir.join("LICENSE");
    let current_year = chrono::Utc::now().format("%Y").to_string();
    let license_content = format!(
        "MIT License\n\n\
         Copyright (c) {} {}\n\n\
         Permission is hereby granted, free of charge, to any person obtaining a copy\n\
         of this software and associated documentation files (the \"Software\"), to deal\n\
         in the Software without restriction, including without limitation the rights\n\
         to use, copy, modify, merge, publish, distribute, sublicense, and/or sell\n\
         copies of the Software, and to permit persons to whom the Software is\n\
         furnished to do so, subject to the following conditions:\n\n\
         The above copyright notice and this permission notice shall be included in all\n\
         copies or substantial portions of the Software.\n\n\
         THE SOFTWARE IS PROVIDED \"AS IS\", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR\n\
         IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,\n\
         FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE\n\
         AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER\n\
         LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,\n\
         OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE\n\
         SOFTWARE.\n",
        current_year, config.created_by
    );
    std::fs::write(&license_path, license_content)
        .map_err(|e| format!("Error al escribir LICENSE: {}", e))?;

    Ok((project_dir, project_uuid))
}

pub fn get_default_workspace_path() -> Result<std::path::PathBuf, String> {
    let home = if cfg!(target_os = "windows") {
        std::env::var("USERPROFILE").ok()
    } else {
        std::env::var("HOME").ok()
    };
    let home_path = home.map(std::path::PathBuf::from)
        .ok_or_else(|| "No se pudo determinar el directorio de inicio (Home) del usuario.".to_string())?;

    if cfg!(target_os = "windows") {
        let candidates = [
            home_path.join("OneDrive").join("Documentos"),
            home_path.join("OneDrive").join("Documents"),
            home_path.join("Documentos"),
            home_path.join("Documents"),
        ];
        for candidate in &candidates {
            if candidate.exists() {
                return Ok(candidate.join("ITO"));
            }
        }
        Ok(home_path.join("Documents").join("ITO"))
    } else {
        let candidates = [
            home_path.join("Documents"),
            home_path.join("Documentos"),
        ];
        for candidate in &candidates {
            if candidate.exists() {
                return Ok(candidate.join("ITO"));
            }
        }
        Ok(home_path.join("Documents").join("ITO"))
    }
}

pub fn get_global_config_pointer_path() -> Result<std::path::PathBuf, String> {
    let home = if cfg!(target_os = "windows") {
        std::env::var("USERPROFILE").ok()
    } else {
        std::env::var("HOME").ok()
    };
    home.map(|h| std::path::PathBuf::from(h).join(".ito").join("config.json"))
        .ok_or_else(|| "No se pudo determinar el directorio de inicio (Home) del usuario.".to_string())
}

pub fn load_workspace_config() -> Result<Option<models::ItoWorkspaceConfig>, String> {
    let pointer_path = get_global_config_pointer_path()?;
    if pointer_path.exists() {
        let content = std::fs::read_to_string(&pointer_path)
            .map_err(|e| format!("Error al leer configuración global en {}: {}", pointer_path.display(), e))?;
        let config: models::ItoWorkspaceConfig = serde_json::from_str(&content)
            .map_err(|e| format!("Error al parsear configuración global: {}", e))?;
        return Ok(Some(config));
    }

    // Si no hay pointer, probar en la ubicación por defecto
    let default_ws = get_default_workspace_path()?;
    let default_config_path = default_ws.join("Config").join("config.json");
    if default_config_path.exists() {
        let content = std::fs::read_to_string(&default_config_path)
            .map_err(|e| format!("Error al leer configuración en {}: {}", default_config_path.display(), e))?;
        let config: models::ItoWorkspaceConfig = serde_json::from_str(&content)
            .map_err(|e| format!("Error al parsear configuración: {}", e))?;
        return Ok(Some(config));
    }

    Ok(None)
}

pub fn save_workspace_config(workspace_path: &std::path::Path) -> Result<(), String> {
    let config = models::ItoWorkspaceConfig {
        workspace: workspace_path.to_string_lossy().to_string(),
        version: "1.0".to_string(),
    };
    let config_str = serde_json::to_string_pretty(&config)
        .map_err(|e| format!("Error al serializar configuración de workspace: {}", e))?;

    // Asegurar subdirectorios del workspace
    initialize_workspace_structure(workspace_path)?;

    // Guardar en Workspace/Config/config.json
    let local_config_path = workspace_path.join("Config").join("config.json");
    std::fs::write(&local_config_path, &config_str)
        .map_err(|e| format!("Error al escribir configuración de workspace en {}: {}", local_config_path.display(), e))?;

    // Guardar en ~/.ito/config.json
    let pointer_path = get_global_config_pointer_path()?;
    if let Some(parent) = pointer_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Error al crear directorio global de configuración {}: {}", parent.display(), e))?;
    }
    std::fs::write(&pointer_path, &config_str)
        .map_err(|e| format!("Error al escribir puntero de configuración en {}: {}", pointer_path.display(), e))?;

    Ok(())
}

pub fn initialize_workspace_structure(workspace_path: &std::path::Path) -> Result<(), String> {
    let subdirs = ["Projects", "Templates", "Cache", "Logs", "Config"];
    for subdir in &subdirs {
        let path = workspace_path.join(subdir);
        std::fs::create_dir_all(&path)
            .map_err(|e| format!("Error al crear subdirectorio del workspace '{}': {}", path.display(), e))?;
    }
    Ok(())
}

pub fn run_workspace_get_count(workspace_path: &std::path::Path) -> usize {
    let projects_dir = workspace_path.join("Projects");
    let mut count = 0;
    if let Ok(entries) = std::fs::read_dir(projects_dir) {
        for entry in entries.flatten() {
            if entry.path().is_dir() {
                count += 1;
            }
        }
    }
    count
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ProjectInfo {
    pub name: String,
    pub path: std::path::PathBuf,
}

pub fn scan_directory_for_projects(dir: &std::path::Path) -> Vec<ProjectInfo> {
    let mut projects = Vec::new();
    
    // Si el directorio en sí mismo es un proyecto
    if dir.join(".ito").is_dir() || dir.join("ito.json").is_file() {
        let name = dir.file_name().and_then(|s| s.to_str()).unwrap_or("unnamed").to_string();
        projects.push(ProjectInfo {
            name,
            path: dir.to_path_buf(),
        });
    }

    // Escanear subdirectorios inmediatos
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if path.join(".ito").is_dir() || path.join("ito.json").is_file() {
                    let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("unnamed").to_string();
                    projects.push(ProjectInfo {
                        name,
                        path,
                    });
                }
            }
        }
    }
    
    // Ordenar alfabéticamente por nombre
    projects.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    projects
}

pub fn copy_to_clipboard(text: &str) {
    use std::process::{Command, Stdio};
    use std::io::Write;
    
    if cfg!(target_os = "windows") {
        if let Ok(mut child) = Command::new("clip").stdin(Stdio::piped()).spawn() {
            if let Some(mut stdin) = child.stdin.take() {
                stdin.write_all(text.as_bytes()).ok();
            }
            child.wait().ok();
        }
    } else if cfg!(target_os = "macos") {
        if let Ok(mut child) = Command::new("pbcopy").stdin(Stdio::piped()).spawn() {
            if let Some(mut stdin) = child.stdin.take() {
                stdin.write_all(text.as_bytes()).ok();
            }
            child.wait().ok();
        }
    } else {
        // En Linux intentamos xclip
        if let Ok(mut child) = Command::new("xclip").arg("-selection").arg("clipboard").stdin(Stdio::piped()).spawn() {
            if let Some(mut stdin) = child.stdin.take() {
                stdin.write_all(text.as_bytes()).ok();
            }
            child.wait().ok();
        }
    }
}

pub fn find_project_root(start_dir: &std::path::Path) -> Option<std::path::PathBuf> {
    // 1. Intentar el método habitual de subir por los directorios padres
    let mut current = start_dir.to_path_buf();
    loop {
        if current.join("ito.json").is_file() || current.join(".ito").join("config.toml").is_file() {
            return Some(current);
        }
        if !current.pop() {
            break;
        }
    }

    // 2. Si no se encuentra, buscar en los proyectos del Workspace de ITO
    if let Ok(Some(ws_config)) = load_workspace_config() {
        let ws_path = std::path::PathBuf::from(&ws_config.workspace);
        let projects_dir = ws_path.join("Projects");
        let projects = scan_directory_for_projects(&projects_dir);
        
        let start_str = start_dir.to_string_lossy().to_lowercase().replace('\\', "/");
        
        for project in projects {
            let ito_json_path = project.path.join("ito.json");
            if ito_json_path.exists() {
                if let Ok(content) = std::fs::read_to_string(&ito_json_path) {
                    if let Ok(config) = serde_json::from_str::<models::ItoProjectConfig>(&content) {
                        if let Some(links) = config.links {
                            for link in links.values() {
                                let link_str = std::path::PathBuf::from(&link.path).to_string_lossy().to_lowercase().replace('\\', "/");
                                if start_str == link_str || start_str.starts_with(&format!("{}/", link_str)) {
                                    return Some(project.path);
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    None
}

pub fn find_linked_module_in_project(project_root: &std::path::Path, current_dir: &std::path::Path) -> Option<(String, String)> {
    let ito_json_path = project_root.join("ito.json");
    if ito_json_path.exists() {
        if let Ok(content) = std::fs::read_to_string(&ito_json_path) {
            if let Ok(config) = serde_json::from_str::<models::ItoProjectConfig>(&content) {
                if let Some(links) = config.links {
                    let modules = [
                        ("firmware", "Firmware"),
                        ("electronics", "Electrónica"),
                        ("mechanical", "Mecánica"),
                        ("documentation", "Documentación"),
                        ("manufacturing", "Manufactura"),
                    ];
                    let current_str = current_dir.to_string_lossy().to_lowercase().replace('\\', "/");
                    for (key, name) in &modules {
                        if let Some(link) = links.get(*key) {
                            let link_str = std::path::PathBuf::from(&link.path).to_string_lossy().to_lowercase().replace('\\', "/");
                            if current_str == link_str || current_str.starts_with(&format!("{}/", link_str)) {
                                return Some((key.to_string(), name.to_string()));
                            }
                        }
                    }
                }
            }
        }
    }
    None
}

pub fn detect_tool_in_path(path: &std::path::Path) -> String {
    if let Ok(entries) = std::fs::read_dir(path) {
        for entry in entries.flatten() {
            let file_name = entry.file_name().to_string_lossy().to_string();
            let file_name_lower = file_name.to_lowercase();
            
            // Visual Studio
            if file_name_lower.ends_with(".sln") {
                return "Visual Studio".to_string();
            }
            // PlatformIO
            if file_name_lower == "platformio.ini" {
                return "PlatformIO".to_string();
            }
            // Arduino
            if file_name_lower.ends_with(".ino") {
                return "Arduino".to_string();
            }
            // KiCad
            if file_name_lower.ends_with(".kicad_pro") || file_name_lower.ends_with(".kicad_pcb") {
                return "KiCad".to_string();
            }
            // Altium
            if file_name_lower.ends_with(".prjpcb") {
                return "Altium Designer".to_string();
            }
            // Proteus
            if file_name_lower.ends_with(".pdsprj") {
                return "Proteus".to_string();
            }
            // Fusion360
            if file_name_lower.ends_with(".f3d") {
                return "Fusion360".to_string();
            }
            // SolidWorks
            if file_name_lower.ends_with(".sldprt") || file_name_lower.ends_with(".sldasm") {
                return "SolidWorks".to_string();
            }
            // FreeCAD
            if file_name_lower.ends_with(".fcstd") {
                return "FreeCAD".to_string();
            }
        }
    }
    
    // Si no se encuentra ninguno en el raíz, buscar carpeta .vscode
    if path.join(".vscode").is_dir() {
        return "Visual Studio Code".to_string();
    }

    "Unknown".to_string()
}

pub fn run_link(project_root: std::path::PathBuf, module_key: &str, target_path: std::path::PathBuf) -> Result<String, String> {
    if !target_path.is_dir() {
        return Err(format!("La ruta especificada '{}' no es un directorio válido o no existe.", target_path.display()));
    }

    let ito_json_path = project_root.join("ito.json");
    if !ito_json_path.exists() {
        return Err("No se encontró el archivo ito.json en el proyecto actual. ¿Inicializaste el proyecto con 'ito init' o 'ito new'?".to_string());
    }

    // Cargar ito.json existente
    let content = std::fs::read_to_string(&ito_json_path)
        .map_err(|e| format!("Error al leer ito.json: {}", e))?;
    let mut config: models::ItoProjectConfig = serde_json::from_str(&content)
        .map_err(|e| format!("Error al parsear ito.json: {}", e))?;

    // Detectar herramienta
    let tool_detected = detect_tool_in_path(&target_path);

    // Detectar motor por defecto según el módulo y herramientas presentes
    let engine_detected = match module_key {
        "firmware" => {
            if target_path.join(".git").is_dir() {
                "git".to_string()
            } else {
                "file-hash".to_string()
            }
        }
        "electronics" => "semantic-cad".to_string(),
        _ => "file-hash".to_string(),
    };

    // Crear enlace
    let link = models::ItoProjectLink {
        path: target_path.to_string_lossy().to_string(),
        tool: tool_detected.clone(),
        engine: engine_detected,
    };

    // Actualizar sección de links
    let mut links_map = config.links.unwrap_or_default();
    links_map.insert(module_key.to_string(), link);
    config.links = Some(links_map);

    // Escribir ito.json actualizado
    let updated_content = serde_json::to_string_pretty(&config)
        .map_err(|e| format!("Error al serializar config: {}", e))?;
    std::fs::write(&ito_json_path, updated_content)
        .map_err(|e| format!("Error al guardar ito.json: {}", e))?;

    Ok(tool_detected)
}

pub fn write_goto_script(cd_command: &str) {
    if let Some(temp_dir) = std::env::var_os("TEMP") {
        let temp_path_ps1 = std::path::Path::new(&temp_dir).join("ito_goto.ps1");
        let _ = std::fs::write(&temp_path_ps1, cd_command);
        
        let temp_path_bat = std::path::Path::new(&temp_dir).join("ito_goto.bat");
        let _ = std::fs::write(&temp_path_bat, cd_command);
    }
}

pub fn open_folder_dialog(description: &str) -> Option<String> {
    let ps_command = format!(
        "Add-Type -AssemblyName System.Windows.Forms; \
         $f = New-Object System.Windows.Forms.FolderBrowserDialog; \
         $f.Description = '{}'; \
         $f.ShowNewFolderButton = $true; \
         if ($f.ShowDialog() -eq [System.Windows.Forms.DialogResult]::OK) {{ $f.SelectedPath }}",
        description
    );

    let output = std::process::Command::new("powershell")
        .args(&["-NoProfile", "-Command", &ps_command])
        .output()
        .ok()?;

    if output.status.success() {
        let path_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !path_str.is_empty() {
            return Some(path_str);
        }
    }
    None
}

pub fn install_shell_wrappers() -> std::result::Result<(), String> {
    if let Ok(current_exe) = std::env::current_exe() {
        if let Some(bin_dir) = current_exe.parent() {
            let old_exe = bin_dir.join("ito.exe");
            if old_exe.exists() {
                let _ = std::fs::remove_file(&old_exe);
            }

            let cmd_path = bin_dir.join("ito.cmd");
            let cmd_content = "@echo off\r\n_ito.exe %*\r\nif exist \"%TEMP%\\ito_goto.bat\" (\r\n    call \"%TEMP%\\ito_goto.bat\"\r\n    del \"%TEMP%\\ito_goto.bat\"\r\n)\r\n";
            let _ = std::fs::write(&cmd_path, cmd_content);
        }
    }

    if let Some(user_profile) = std::env::var_os("USERPROFILE") {
        let user_profile_path = std::path::Path::new(&user_profile);
        
        let profile_dirs = [
            user_profile_path.join("Documents").join("WindowsPowerShell"),
            user_profile_path.join("OneDrive").join("Documents").join("WindowsPowerShell"),
            user_profile_path.join("Documents").join("PowerShell"),
            user_profile_path.join("OneDrive").join("Documents").join("PowerShell"),
        ];

        let wrapper_code = "\r\nfunction ito {\r\n    & _ito.exe $args\r\n    if (Test-Path \"$env:TEMP\\ito_goto.ps1\") {\r\n        . \"$env:TEMP\\ito_goto.ps1\"\r\n        Remove-Item \"$env:TEMP\\ito_goto.ps1\"\r\n    }\r\n}\r\n";

        for dir in &profile_dirs {
            let profile_file = dir.join("Microsoft.PowerShell_profile.ps1");
            if dir.exists() || std::fs::create_dir_all(dir).is_ok() {
                let mut content = if profile_file.exists() {
                    std::fs::read_to_string(&profile_file).unwrap_or_default()
                } else {
                    String::new()
                };

                if content.contains("& ito.exe $args") {
                    content = content.replace("& ito.exe $args", "& _ito.exe $args");
                    let _ = std::fs::write(&profile_file, &content);
                } else if !content.contains("function ito {") {
                    content.push_str(wrapper_code);
                    let _ = std::fs::write(&profile_file, content);
                }
            }
        }
    }

    Ok(())
}

pub fn run_auth_login(project_dir: std::path::PathBuf, token: &str) -> std::result::Result<(), String> {
    let ito_dir = project_dir.join(".ito");
    if !ito_dir.exists() {
        return Err("No se encontró el directorio .ito. ¿Inicializaste el proyecto con 'ito init' o 'ito new'?".to_string());
    }

    let config_path = ito_dir.join("config.toml");
    let mut config = if config_path.exists() {
        let content = std::fs::read_to_string(&config_path)
            .map_err(|e| format!("Error al leer configuración: {}", e))?;
        toml::from_str::<Config>(&content)
            .map_err(|e| format!("Error al parsear configuración: {}", e))?
    } else {
        Config {
            project_id: project_dir.file_name().unwrap_or_default().to_string_lossy().to_string(),
            remote_url: "https://api.alexandria-hq.com/v1/reports".to_string(),
            token: None,
        }
    };

    config.token = Some(token.to_string());
    if token.starts_with("ito_tk_") {
        config.remote_url = "https://itogravity.com/php/ito_api.php".to_string();
    }

    // Si es itogravity, validar y obtener project_id y project_name reales
    if config.remote_url.contains("itogravity.com") {
        let client = reqwest::blocking::Client::new();
        let mut params = std::collections::HashMap::new();
        params.insert("action", "info");
        params.insert("token", token);

        println!("Conectando con ITOGravity para validar credenciales...");
        let response = client.post(&config.remote_url)
            .form(&params)
            .send()
            .map_err(|e| format!("Error de conexión al servidor: {}", e))?;

        if !response.status().is_success() {
            return Err("Token inválido o expirado. Verifica tus credenciales.".to_string());
        }

        let resp_json: serde_json::Value = response.json()
            .map_err(|e| format!("Error al decodificar respuesta del servidor: {}", e))?;

        if let Some(proj_id) = resp_json.get("project_id") {
            let id_str = match proj_id {
                serde_json::Value::Number(n) => n.to_string(),
                serde_json::Value::String(s) => s.clone(),
                _ => return Err("Formato de ID de proyecto inválido recibido.".to_string()),
            };
            config.project_id = id_str;
        }

        // Crear/Actualizar ito.json para asegurar congruencia del proyecto local
        let ito_json_path = project_dir.join("ito.json");
        let project_name = resp_json.get("project_name")
            .and_then(|n| n.as_str())
            .unwrap_or("Proyecto Sincronizado");

        let mut ito_config = if ito_json_path.exists() {
            if let Ok(c) = std::fs::read_to_string(&ito_json_path) {
                serde_json::from_str::<models::ItoProjectConfig>(&c).unwrap_or_else(|_| models::ItoProjectConfig {
                    format_version: "1.0".to_string(),
                    project_name: project_name.to_string(),
                    project_uuid: uuid::Uuid::new_v4().to_string(),
                    created_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
                    created_by: "ITO CLI".to_string(),
                    modules: models::ItoProjectModules {
                        firmware: true,
                        electronics: true,
                        mechanical: true,
                        documentation: true,
                        manufacturing: true,
                    },
                    current_revision: "REV-0001".to_string(),
                    license: "MIT".to_string(),
                    version: "0.1.0".to_string(),
                    links: None,
                })
            } else {
                models::ItoProjectConfig {
                    format_version: "1.0".to_string(),
                    project_name: project_name.to_string(),
                    project_uuid: uuid::Uuid::new_v4().to_string(),
                    created_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
                    created_by: "ITO CLI".to_string(),
                    modules: models::ItoProjectModules {
                        firmware: true,
                        electronics: true,
                        mechanical: true,
                        documentation: true,
                        manufacturing: true,
                    },
                    current_revision: "REV-0001".to_string(),
                    license: "MIT".to_string(),
                    version: "0.1.0".to_string(),
                    links: None,
                }
            }
        } else {
            models::ItoProjectConfig {
                format_version: "1.0".to_string(),
                project_name: project_name.to_string(),
                project_uuid: uuid::Uuid::new_v4().to_string(),
                created_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
                created_by: "ITO CLI".to_string(),
                modules: models::ItoProjectModules {
                    firmware: true,
                    electronics: true,
                    mechanical: true,
                    documentation: true,
                    manufacturing: true,
                },
                current_revision: "REV-0001".to_string(),
                license: "MIT".to_string(),
                version: "0.1.0".to_string(),
                links: None,
            }
        };

        ito_config.project_name = project_name.to_string();
        if let Ok(c_json) = serde_json::to_string_pretty(&ito_config) {
            let _ = std::fs::write(&ito_json_path, c_json);
        }
    }

    let toml_str = toml::to_string_pretty(&config)
        .map_err(|e| format!("Error al serializar configuración: {}", e))?;
    std::fs::write(&config_path, toml_str)
        .map_err(|e| format!("Error al escribir configuración: {}", e))?;

    Ok(())
}

pub fn get_latest_design_json(project_dir: &std::path::Path) -> std::result::Result<(String, Option<String>), String> {
    let mut target_dir = None;

    // 1. Intentar resolver usando links en ito.json
    let ito_json_path = project_dir.join("ito.json");
    if ito_json_path.exists() {
        if let Ok(content) = std::fs::read_to_string(&ito_json_path) {
            if let Ok(config) = serde_json::from_str::<models::ItoProjectConfig>(&content) {
                if let Some(links) = config.links {
                    if let Some(link) = links.get("electronics") {
                        let path = std::path::PathBuf::from(&link.path);
                        if path.exists() {
                            target_dir = Some(path);
                        }
                    }
                }
            }
        }
    }

    // 2. Si no está en links, verificar si existe subcarpeta electronics en el raíz
    if target_dir.is_none() {
        let local_electronics = project_dir.join("electronics");
        if local_electronics.exists() {
            target_dir = Some(local_electronics);
        }
    }

    // 3. Fallback a cache/electronics
    if target_dir.is_none() {
        let cache_electronics = project_dir.join(".ito").join("cache").join("electronics");
        if cache_electronics.exists() {
            target_dir = Some(cache_electronics);
        }
    }

    // 4. Fallback final al raíz del proyecto
    let target_dir = target_dir.unwrap_or_else(|| project_dir.to_path_buf());

    let design = parsers::parse_project_directory(&target_dir)
        .unwrap_or_else(|_| models::HardwareDesign::new());

    let design_json = serde_json::to_string(&design)
        .map_err(|e| format!("Error al serializar el diseño a JSON: {}", e))?;

    let mut bom_csv = None;
    if let Ok(entries) = std::fs::read_dir(&target_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() {
                if let Some(ext) = path.extension().and_then(|s| s.to_str()) {
                    if ext.to_lowercase() == "csv" && path.file_name().and_then(|s| s.to_str()).unwrap_or("").to_lowercase().contains("bom") {
                        if let Ok(content) = std::fs::read_to_string(&path) {
                            bom_csv = Some(content);
                        }
                        break;
                    }
                }
            }
        }
    }

    Ok((design_json, bom_csv))
}

pub fn create_project_zip(project_dir: &std::path::Path) -> std::result::Result<Vec<u8>, String> {
    use std::io::Write;

    // Asegurar que las carpetas estándar vacías tengan un archivo .gitkeep para que se suban y se muestren en la web
    let standard_dirs = [
        "firmware",
        "electronics",
        "electronics/pcb",
        "electronics/schematics",
        "electronics/libraries",
        "mechanical",
        "mechanical/cad",
        "mechanical/drawings",
        "documentation",
        "manufacturing",
    ];
    for sub_dir in &standard_dirs {
        let path = project_dir.join(sub_dir);
        if path.exists() && path.is_dir() {
            if let Ok(entries) = std::fs::read_dir(&path) {
                if entries.flatten().count() == 0 {
                    let keep_path = path.join(".gitkeep");
                    let _ = std::fs::write(&keep_path, "# Mantenido vacío por ITO\n");
                }
            }
        }
    }

    let mut links = std::collections::HashMap::new();
    let ito_json_path = project_dir.join("ito.json");
    if ito_json_path.exists() {
        if let Ok(content) = std::fs::read_to_string(&ito_json_path) {
            if let Ok(config) = serde_json::from_str::<models::ItoProjectConfig>(&content) {
                links = config.links.unwrap_or_default();
            }
        }
    }

    let filter = ignore::IgnoreFilter::new(project_dir);
    let mut buffer = Vec::new();
    {
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut buffer));
        let options = zip::write::FileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);

        fn walk_and_zip_dir(
            dir: &std::path::Path,
            base_dir: &std::path::Path,
            prefix_in_zip: &str,
            filter: &ignore::IgnoreFilter,
            zip: &mut zip::ZipWriter<std::io::Cursor<&mut Vec<u8>>>,
            options: zip::write::FileOptions,
            links: &std::collections::HashMap<String, models::ItoProjectLink>,
            is_root_walk: bool,
        ) -> std::result::Result<(), String> {
            if let Ok(entries) = std::fs::read_dir(dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    let relative_path = path.strip_prefix(base_dir)
                        .map_err(|e| format!("Error de ruta: {}", e))?;
                    
                    if filter.is_ignored(&relative_path) {
                        continue;
                    }

                    if path.is_dir() {
                        let dir_name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
                        
                        if is_root_walk && links.contains_key(dir_name) {
                            continue;
                        }

                        walk_and_zip_dir(&path, base_dir, prefix_in_zip, filter, zip, options, links, is_root_walk)?;
                    } else if path.is_file() {
                        let rel_str = relative_path.to_string_lossy().replace('\\', "/");
                        let file_name_in_zip = if prefix_in_zip.is_empty() {
                            rel_str
                        } else {
                            format!("{}/{}", prefix_in_zip, rel_str)
                        };

                        zip.start_file(&file_name_in_zip, options)
                            .map_err(|e| format!("Error al iniciar archivo zip: {}", e))?;
                        
                        let content = std::fs::read(&path)
                            .map_err(|e| format!("Error al leer archivo {}: {}", path.display(), e))?;
                        zip.write_all(&content)
                            .map_err(|e| format!("Error al escribir archivo en zip: {}", e))?;
                    }
                }
            }
            Ok(())
        }

        walk_and_zip_dir(project_dir, project_dir, "", &filter, &mut zip, options, &links, true)?;

        for (module_name, link) in &links {
            let external_path = std::path::Path::new(&link.path);
            if external_path.exists() && external_path.is_dir() {
                let ext_filter = ignore::IgnoreFilter::new(external_path);
                walk_and_zip_dir(external_path, external_path, module_name, &ext_filter, &mut zip, options, &links, false)?;
            }
        }

        zip.finish()
            .map_err(|e| format!("Error al finalizar archivo zip: {}", e))?;
    }
    Ok(buffer)
}

pub async fn run_push(project_dir: std::path::PathBuf) -> std::result::Result<String, String> {
    let ito_dir = project_dir.join(".ito");
    let config_path = ito_dir.join("config.toml");
    if !config_path.exists() {
        return Err("No se encontró el archivo de configuración. Inicializá el proyecto con 'ito init' o 'ito new'.".to_string());
    }
    let content = std::fs::read_to_string(&config_path)
        .map_err(|e| format!("Error al leer configuración: {}", e))?;
    let config: Config = toml::from_str(&content)
        .map_err(|e| format!("Error al parsear configuración: {}", e))?;

    let token = config.token.ok_or_else(|| "No estás autenticado. Ejecutá: ito auth login --token <TOKEN>".to_string())?;

    let history_path = ito_dir.join("history.toml");
    if !history_path.exists() {
        return Err("No hay historial local de commits. Ejecutá 'ito commit' primero.".to_string());
    }
    let hist_content = std::fs::read_to_string(&history_path)
        .map_err(|e| format!("Error al leer historial: {}", e))?;
    let history: History = toml::from_str(&hist_content)
        .map_err(|e| format!("Error al parsear historial: {}", e))?;

    let latest_commit = history.commits.last()
        .ok_or_else(|| "No hay commits locales para enviar. Ejecutá 'ito commit' primero.".to_string())?;

    let (design_json, bom_csv) = get_latest_design_json(&project_dir)?;

    println!("Empaquetando directorios del proyecto...");
    let project_zip_bytes = create_project_zip(&project_dir)?;

    let client = reqwest::Client::new();
    
    let mut form = reqwest::multipart::Form::new()
        .text("project_id", config.project_id.clone())
        .text("domain", "hardware")
        .text("version_hash", latest_commit.hash.clone())
        .text("parent_hash", latest_commit.parent_hash.clone())
        .text("message", latest_commit.message.clone())
        .text("token", token.clone());

    let design_part = reqwest::multipart::Part::text(design_json)
        .file_name("design.json")
        .mime_str("application/json")
        .map_err(|e| format!("Error al preparar archivo de diseño: {}", e))?;
    form = form.part("design_json", design_part);

    if let Some(bom) = bom_csv {
        let bom_part = reqwest::multipart::Part::text(bom)
            .file_name("bom.csv")
            .mime_str("text/csv")
            .map_err(|e| format!("Error al preparar archivo BOM: {}", e))?;
        form = form.part("bom_csv", bom_part);
    }

    let zip_part = reqwest::multipart::Part::bytes(project_zip_bytes)
        .file_name("project_files.zip")
        .mime_str("application/zip")
        .map_err(|e| format!("Error al preparar archivo ZIP del proyecto: {}", e))?;
    form = form.part("project_zip", zip_part);

    println!("Subiendo versión {} a {}...", &latest_commit.hash[..8], &config.remote_url);
    let response = client.post(&config.remote_url)
        .multipart(form)
        .send()
        .await
        .map_err(|e| format!("Error de conexión con el servidor: {}", e))?;

    let status = response.status();
    let body = response.text().await
        .map_err(|e| format!("Error al leer respuesta del servidor: {}", e))?;

    if status.is_success() {
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&body) {
            if let Some(msg) = json.get("message").and_then(|m| m.as_str()) {
                return Ok(msg.to_string());
            }
        }
        Ok("Versión sincronizada exitosamente con el servidor.".to_string())
    } else {
        let mut err_msg = format!("El servidor respondió con código {}", status);
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&body) {
            if let Some(msg) = json.get("message").and_then(|m| m.as_str()) {
                err_msg = msg.to_string();
            }
        }
        Err(err_msg)
    }
}

pub async fn run_pull(project_dir: std::path::PathBuf) -> std::result::Result<String, String> {
    let ito_dir = project_dir.join(".ito");
    let config_path = ito_dir.join("config.toml");
    if !config_path.exists() {
        return Err("No se encontró el archivo de configuración. Inicializá el proyecto con 'ito init' o 'ito new'.".to_string());
    }
    let content = std::fs::read_to_string(&config_path)
        .map_err(|e| format!("Error al leer configuración: {}", e))?;
    let config: Config = toml::from_str(&content)
        .map_err(|e| format!("Error al parsear configuración: {}", e))?;

    let token = config.token.ok_or_else(|| "No estás autenticado. Ejecutá: ito auth login --token <TOKEN>".to_string())?;

    let client = reqwest::Client::new();
    let mut params = std::collections::HashMap::new();
    params.insert("action", "pull");
    params.insert("project_id", &config.project_id);
    params.insert("token", &token);

    println!("Consultando última versión en {}...", &config.remote_url);
    let response = client.post(&config.remote_url)
        .form(&params)
        .send()
        .await
        .map_err(|e| format!("Error de conexión con el servidor: {}", e))?;

    let status = response.status();
    let body = response.text().await
        .map_err(|e| format!("Error al leer respuesta del servidor: {}", e))?;

    if !status.is_success() {
        let mut err_msg = format!("El servidor respondió con código {}", status);
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&body) {
            if let Some(msg) = json.get("message").and_then(|m| m.as_str()) {
                err_msg = msg.to_string();
            }
        }
        return Err(err_msg);
    }

    let json_resp: serde_json::Value = serde_json::from_str(&body)
        .map_err(|e| format!("Error al decodificar respuesta JSON: {}", e))?;

    let version_hash = json_resp.get("version_hash").and_then(|h| h.as_str())
        .ok_or_else(|| "El servidor no retornó un hash de versión válido.".to_string())?;
    
    let parent_hash = json_resp.get("parent_hash").and_then(|h| h.as_str()).unwrap_or("");
    let message = json_resp.get("message").and_then(|m| m.as_str()).unwrap_or("Sincronización remota");
    
    let history_path = ito_dir.join("history.toml");
    let mut history = if history_path.exists() {
        let hist_content = std::fs::read_to_string(&history_path)
            .map_err(|e| format!("Error al leer historial: {}", e))?;
        toml::from_str::<History>(&hist_content).unwrap_or_default()
    } else {
        History::default()
    };

    if let Some(latest_local) = history.commits.last() {
        if latest_local.hash == version_hash {
            return Ok(format!("Ya estás actualizado a la última versión del servidor ({}).", &version_hash[..8]));
        }
    }

    println!("Descargando versión completa {} desde el servidor...", &version_hash[..8]);
    let mut zip_params = std::collections::HashMap::new();
    zip_params.insert("action", "download_zip");
    zip_params.insert("project_id", &config.project_id);
    zip_params.insert("token", &token);
    zip_params.insert("version_hash", version_hash);

    let zip_response = client.post(&config.remote_url)
        .form(&zip_params)
        .send()
        .await
        .map_err(|e| format!("Error al descargar el paquete del proyecto: {}", e))?;

    let zip_status = zip_response.status();
    if !zip_status.is_success() {
        let zip_err_body = zip_response.text().await.unwrap_or_default();
        let mut err_msg = format!("El servidor respondió con código {} al descargar el ZIP.", zip_status);
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&zip_err_body) {
            if let Some(msg) = json.get("message").and_then(|m| m.as_str()) {
                err_msg = msg.to_string();
            }
        }
        return Err(err_msg);
    }

    let zip_bytes = zip_response.bytes().await
        .map_err(|e| format!("Error al leer los bytes del ZIP: {}", e))?;

    println!("Extrayendo archivos del proyecto...");
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(zip_bytes))
        .map_err(|e| format!("Error al abrir archivo ZIP: {}", e))?;

    for i in 0..archive.len() {
        let mut file = archive.by_index(i)
            .map_err(|e| format!("Error al leer entrada del ZIP: {}", e))?;
        let outpath = match file.enclosed_name() {
            Some(path) => project_dir.join(path),
            None => continue,
        };

        if file.name().ends_with('/') {
            std::fs::create_dir_all(&outpath).ok();
        } else {
            if let Some(p) = outpath.parent() {
                std::fs::create_dir_all(p).ok();
            }
            let mut outfile = std::fs::File::create(&outpath)
                .map_err(|e| format!("Error al crear archivo local: {}", e))?;
            std::io::copy(&mut file, &mut outfile)
                .map_err(|e| format!("Error al escribir archivo: {}", e))?;
        }
    }

    let mut electronics_path = project_dir.clone();
    let ito_json_path = project_dir.join("ito.json");
    if ito_json_path.exists() {
        if let Ok(c) = std::fs::read_to_string(&ito_json_path) {
            if let Ok(cfg) = serde_json::from_str::<models::ItoProjectConfig>(&c) {
                if let Some(links) = cfg.links {
                    if let Some(link) = links.get("electronics") {
                        electronics_path = std::path::PathBuf::from(&link.path);
                    }
                }
            }
        }
    }

    let design_json_val = json_resp.get("design_json");
    let bom_csv_val = json_resp.get("bom_csv");

    let cache_dir = ito_dir.join("cache").join("electronics");
    std::fs::create_dir_all(&cache_dir).ok();
    std::fs::create_dir_all(&electronics_path).ok();

    if let Some(design_obj) = design_json_val {
        if !design_obj.is_null() {
            let design_str = serde_json::to_string_pretty(design_obj).unwrap_or_default();
            std::fs::write(electronics_path.join("design.json"), &design_str).ok();
            std::fs::write(cache_dir.join("design.json"), &design_str).ok();
        }
    }

    if let Some(bom_obj) = bom_csv_val {
        if let Some(bom_str) = bom_obj.as_str() {
            std::fs::write(electronics_path.join("bom.csv"), bom_str).ok();
            std::fs::write(cache_dir.join("bom.csv"), bom_str).ok();
        }
    }

    let mut manifest_hashes = std::collections::HashMap::new();
    if electronics_path.join("design.json").exists() {
        if let Ok(h) = cas::calculate_file_hash(&electronics_path.join("design.json")) {
            manifest_hashes.insert("design.json".to_string(), h);
        }
    }
    if electronics_path.join("bom.csv").exists() {
        if let Ok(h) = cas::calculate_file_hash(&electronics_path.join("bom.csv")) {
            manifest_hashes.insert("bom.csv".to_string(), h);
        }
    }
    let manifest_str = serde_json::to_string_pretty(&manifest_hashes).unwrap_or_default();
    std::fs::write(cache_dir.join("manifest.json"), &manifest_str).ok();

    let timestamp = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    
    let mut diff_summary = None;
    if let Some(design_obj) = design_json_val {
        if !design_obj.is_null() {
            if let Ok(new_design) = serde_json::from_value::<models::HardwareDesign>(design_obj.clone()) {
                diff_summary = Some(DiffSummary {
                    added_components: new_design.components.len(),
                    deleted_components: 0,
                    modified_components: 0,
                    added_nets: new_design.nets.len(),
                    deleted_nets: 0,
                    modified_nets: 0,
                });
            }
        }
    }

    let commit_entry = CommitEntry {
        hash: version_hash.to_string(),
        parent_hash: parent_hash.to_string(),
        message: message.to_string(),
        timestamp,
        zip_path: format!(".ito/backups/{}", version_hash),
        synced: true,
        diff_summary,
        modules: std::collections::HashMap::new(),
    };

    history.commits.push(commit_entry);
    let history_str = toml::to_string_pretty(&history)
        .map_err(|e| format!("Error al serializar historial: {}", e))?;
    std::fs::write(&history_path, history_str)
        .map_err(|e| format!("Error al escribir historial: {}", e))?;

    Ok(format!("Descargada e integrada versión {} ({})", &version_hash[..8], message))
}

#[cfg(test)]
mod tests {
    use super::*;
    static ENV_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn test_run_new_creation() {
        let unique_id = uuid::Uuid::new_v4().to_string();
        let temp_dir = std::env::temp_dir().join(format!("ito-test-{}", unique_id));
        let project_name = "TestProject";
        
        let (project_path, uuid) = run_new(temp_dir.clone(), project_name).unwrap();
        
        assert_eq!(project_path, temp_dir.join(project_name));
        assert!(!uuid.is_empty());
        
        // Verificar carpetas
        assert!(project_path.join("firmware").is_dir());
        assert!(project_path.join("electronics/pcb").is_dir());
        assert!(project_path.join("mechanical/cad").is_dir());
        assert!(project_path.join(".ito/backups").is_dir());
        
        // Verificar archivos
        assert!(project_path.join("ito.json").is_file());
        assert!(project_path.join("README.md").is_file());
        assert!(project_path.join("LICENSE").is_file());
        
        // Intentar crear el mismo proyecto de nuevo debe dar error
        let err_res = run_new(temp_dir.clone(), project_name);
        assert!(err_res.is_err());

        // Limpiar
        std::fs::remove_dir_all(&temp_dir).ok();
    }

    #[test]
    fn test_workspace_config_flow() {
        let _guard = ENV_MUTEX.lock().unwrap();
        let unique_id = uuid::Uuid::new_v4().to_string();
        let temp_home = std::env::temp_dir().join(format!("ito-home-{}", unique_id));
        let temp_ws = temp_home.join("Documents").join("ITO");

        // Configurar variables de entorno temporales para aislar la prueba
        let original_userprofile = std::env::var("USERPROFILE").ok();
        let original_home = std::env::var("HOME").ok();

        std::env::set_var("USERPROFILE", &temp_home);
        std::env::set_var("HOME", &temp_home);

        // Validar rutas por defecto
        let default_ws = get_default_workspace_path().unwrap();
        assert_eq!(default_ws, temp_ws);

        // Inicializar estructura del workspace
        initialize_workspace_structure(&temp_ws).unwrap();
        assert!(temp_ws.join("Projects").is_dir());
        assert!(temp_ws.join("Config").is_dir());

        // Guardar configuración
        save_workspace_config(&temp_ws).unwrap();
        
        // Cargar configuración
        let loaded = load_workspace_config().unwrap();
        assert!(loaded.is_some());
        let config = loaded.unwrap();
        assert_eq!(config.workspace, temp_ws.to_string_lossy().to_string());

        // Contar proyectos en workspace mock
        assert_eq!(run_workspace_get_count(&temp_ws), 0);
        
        // Crear un proyecto de prueba
        let projects_dir = temp_ws.join("Projects");
        let (p_path, _) = run_new(projects_dir, "Proj1").unwrap();
        assert!(p_path.is_dir());
        assert_eq!(run_workspace_get_count(&temp_ws), 1);

        // Limpiar
        std::fs::remove_dir_all(&temp_home).ok();

        // Restaurar variables de entorno
        if let Some(val) = original_userprofile {
            std::env::set_var("USERPROFILE", val);
        } else {
            std::env::remove_var("USERPROFILE");
        }
        if let Some(val) = original_home {
            std::env::set_var("HOME", val);
        } else {
            std::env::remove_var("HOME");
        }
    }

    #[test]
    fn test_scan_directory_for_projects() {
        let unique_id = uuid::Uuid::new_v4().to_string();
        let temp_dir = std::env::temp_dir().join(format!("ito-scan-{}", unique_id));
        std::fs::create_dir_all(&temp_dir).unwrap();

        // 1. Crear subdirectorio que sea proyecto
        let proj1_dir = temp_dir.join("Proj1");
        std::fs::create_dir_all(proj1_dir.join(".ito")).unwrap();

        // 2. Crear subdirectorio que no sea proyecto
        let non_proj_dir = temp_dir.join("NonProj");
        std::fs::create_dir_all(non_proj_dir).unwrap();

        // Escanear
        let projects = scan_directory_for_projects(&temp_dir);
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].name, "Proj1");

        // Limpiar
        std::fs::remove_dir_all(&temp_dir).ok();
    }

    #[test]
    fn test_project_root_discovery() {
        let unique_id = uuid::Uuid::new_v4().to_string();
        let temp_dir = std::env::temp_dir().join(format!("ito-root-{}", unique_id));
        let nested_dir = temp_dir.join("a").join("b").join("c");
        std::fs::create_dir_all(&nested_dir).unwrap();

        // No hay proyecto todavía
        assert!(find_project_root(&nested_dir).is_none());

        // Crear ito.json en la raíz temporal
        std::fs::write(temp_dir.join("ito.json"), "{}").unwrap();
        
        let found = find_project_root(&nested_dir);
        assert!(found.is_some());
        assert_eq!(found.unwrap(), temp_dir);

        // Limpiar
        std::fs::remove_dir_all(&temp_dir).ok();
    }

    #[test]
    fn test_tool_detection() {
        let unique_id = uuid::Uuid::new_v4().to_string();
        let temp_dir = std::env::temp_dir().join(format!("ito-detect-{}", unique_id));
        std::fs::create_dir_all(&temp_dir).unwrap();

        // 1. Caso Arduino
        let arduino_dir = temp_dir.join("arduino");
        std::fs::create_dir_all(&arduino_dir).unwrap();
        std::fs::write(arduino_dir.join("sketch.ino"), "").unwrap();
        assert_eq!(detect_tool_in_path(&arduino_dir), "Arduino");

        // 2. Caso KiCad
        let kicad_dir = temp_dir.join("kicad");
        std::fs::create_dir_all(&kicad_dir).unwrap();
        std::fs::write(kicad_dir.join("pcb.kicad_pcb"), "").unwrap();
        assert_eq!(detect_tool_in_path(&kicad_dir), "KiCad");

        // 3. Caso VS Code
        let vscode_dir = temp_dir.join("vscode");
        std::fs::create_dir_all(vscode_dir.join(".vscode")).unwrap();
        assert_eq!(detect_tool_in_path(&vscode_dir), "Visual Studio Code");

        // 4. Caso Desconocido
        let unknown_dir = temp_dir.join("unknown");
        std::fs::create_dir_all(&unknown_dir).unwrap();
        assert_eq!(detect_tool_in_path(&unknown_dir), "Unknown");

        // Limpiar
        std::fs::remove_dir_all(&temp_dir).ok();
    }

    #[test]
    fn test_project_linking() {
        let unique_id = uuid::Uuid::new_v4().to_string();
        let temp_dir = std::env::temp_dir().join(format!("ito-link-{}", unique_id));
        std::fs::create_dir_all(&temp_dir).unwrap();

        // Crear estructura estándar de nuevo proyecto de ITO
        let (p_path, _) = run_new(temp_dir.clone(), "MyProj").unwrap();

        // Crear una carpeta de firmware simulada con PlatformIO
        let fw_path = temp_dir.join("MyFirmware");
        std::fs::create_dir_all(&fw_path).unwrap();
        std::fs::write(fw_path.join("platformio.ini"), "").unwrap();

        // Ejecutar link
        let tool = run_link(p_path.clone(), "firmware", fw_path.clone()).unwrap();
        assert_eq!(tool, "PlatformIO");

        // Cargar config y verificar
        let config_content = std::fs::read_to_string(p_path.join("ito.json")).unwrap();
        let config: models::ItoProjectConfig = serde_json::from_str(&config_content).unwrap();
        assert!(config.links.is_some());
        let links = config.links.unwrap();
        assert!(links.contains_key("firmware"));
        let link = links.get("firmware").unwrap();
        assert_eq!(link.path, fw_path.to_string_lossy().to_string());
        assert_eq!(link.tool, "PlatformIO");

        // Limpiar
        std::fs::remove_dir_all(&temp_dir).ok();
    }

    #[test]
    fn test_run_commit_local_vcs() {
        let unique_id = uuid::Uuid::new_v4().to_string();
        let temp_dir = std::env::temp_dir().join(format!("ito-vcs-commit-{}", unique_id));
        std::fs::create_dir_all(&temp_dir).unwrap();

        // 1. Crear proyecto
        let (p_path, _) = run_new(temp_dir.clone(), "VcsProj").unwrap();

        // 2. Crear archivo de diseño en la raíz del proyecto para que run_commit lo procese
        let cad_path = p_path.join("design.json");
        std::fs::write(&cad_path, r#"{"components":[], "nets":[]}"#).unwrap();

        // 3. Ejecutar commit
        let commit = run_commit(p_path.clone(), Some("Primer commit".to_string())).unwrap();
        assert_eq!(commit.message, "Primer commit");
        assert!(commit.diff_summary.is_some());
        
        let summary = commit.diff_summary.unwrap();
        assert_eq!(summary.added_components, 0);

        // 4. Modificar diseño para el segundo commit (Añadir R1)
        std::fs::write(&cad_path, r#"{"components":[{"designator":"R1","footprint":"","pins":[]}], "nets":[]}"#).unwrap();
        
        let commit2 = run_commit(p_path.clone(), Some("Segundo commit".to_string())).unwrap();
        assert_eq!(commit2.message, "Segundo commit");
        let summary2 = commit2.diff_summary.unwrap();
        assert_eq!(summary2.added_components, 1); // Detecta que se añadió R1!

        // Limpiar
        std::fs::remove_dir_all(&temp_dir).ok();
    }

    #[test]
    fn test_run_restore_local_vcs() {
        let unique_id = uuid::Uuid::new_v4().to_string();
        let temp_dir = std::env::temp_dir().join(format!("ito-vcs-restore-{}", unique_id));
        std::fs::create_dir_all(&temp_dir).unwrap();

        // 1. Crear proyecto
        let (p_path, _) = run_new(temp_dir.clone(), "RestoreProj").unwrap();

        // 2. Crear primer diseño (R1) y hacer commit
        let cad_path = p_path.join("design.json");
        std::fs::write(&cad_path, r#"{"components":[{"designator":"R1","footprint":"","pins":[]}], "nets":[]}"#).unwrap();
        let commit1 = run_commit(p_path.clone(), Some("Commit 1".to_string())).unwrap();

        // 3. Modificar diseño a (R2) y hacer commit
        std::fs::write(&cad_path, r#"{"components":[{"designator":"R2","footprint":"","pins":[]}], "nets":[]}"#).unwrap();
        let _commit2 = run_commit(p_path.clone(), Some("Commit 2".to_string())).unwrap();

        // 4. Restaurar al primer commit
        let restored = run_restore(p_path.clone(), &commit1.hash[..8]).unwrap();
        assert_eq!(restored.len(), 1);
        assert_eq!(restored[0], "electronics");

        // 5. Leer el archivo restaurado en disco y verificar que contiene R1 (no R2)
        let content = std::fs::read_to_string(&cad_path).unwrap();
        assert!(content.contains("R1"));
        assert!(!content.contains("R2"));

        // Limpiar
        std::fs::remove_dir_all(&temp_dir).ok();
    }

    #[test]
    fn test_intelligent_project_root_resolver() {
        let _guard = ENV_MUTEX.lock().unwrap();
        let unique_id = uuid::Uuid::new_v4().to_string();
        let temp_dir = std::env::temp_dir().join(format!("ito-vcs-intel-{}", unique_id));
        std::fs::create_dir_all(&temp_dir).unwrap();

        // Simular variables de entorno para que apunten al directorio temporal
        let original_userprofile = std::env::var("USERPROFILE").ok();
        let original_home = std::env::var("HOME").ok();

        std::env::set_var("USERPROFILE", &temp_dir);
        std::env::set_var("HOME", &temp_dir);

        // Crear la carpeta del Workspace simulado
        let ws_path = temp_dir.join("Documents").join("ITO");
        std::fs::create_dir_all(&ws_path).unwrap();
        save_workspace_config(&ws_path).unwrap();

        // 1. Crear proyecto del Workspace
        let projects_dir = ws_path.join("Projects");
        let (p_path, _) = run_new(projects_dir, "MyIntelProj").unwrap();

        // 2. Crear una carpeta externa simulada de diseño
        let external_cad_dir = temp_dir.join("MyExternalCAD");
        std::fs::create_dir_all(&external_cad_dir).unwrap();

        // Vincular el módulo a la carpeta externa
        let _ = run_link(p_path.clone(), "electronics", external_cad_dir.clone()).unwrap();

        let resolved_root = find_project_root(&external_cad_dir);
        assert!(resolved_root.is_some());
        let r_root = resolved_root.unwrap();
        assert_eq!(
            r_root.to_string_lossy().to_lowercase().replace('\\', "/"),
            p_path.to_string_lossy().to_lowercase().replace('\\', "/")
        );

        // 4. Probar que find_linked_module_in_project funciona
        let linked_module = find_linked_module_in_project(&p_path, &external_cad_dir);
        assert!(linked_module.is_some());
        let (key, name) = linked_module.unwrap();
        assert_eq!(key, "electronics");
        assert_eq!(name, "Electrónica");

        // Limpiar
        std::fs::remove_dir_all(&temp_dir).ok();

        // Restaurar variables de entorno
        if let Some(val) = original_userprofile {
            std::env::set_var("USERPROFILE", val);
        } else {
            std::env::remove_var("USERPROFILE");
        }
        if let Some(val) = original_home {
            std::env::set_var("HOME", val);
        } else {
            std::env::remove_var("HOME");
        }
    }

    #[test]
    fn test_modular_v2_flow() {
        let unique_id = uuid::Uuid::new_v4().to_string();
        let temp_dir = std::env::temp_dir().join(format!("ito-v2-flow-{}", unique_id));
        std::fs::create_dir_all(&temp_dir).unwrap();

        // 1. Crear proyecto
        let (p_path, _) = run_new(temp_dir.clone(), "V2Proj").unwrap();

        // 2. Crear carpetas externas simuladas
        let fw_src_dir = temp_dir.join("my-firmware");
        let mech_src_dir = temp_dir.join("my-mechanics");
        std::fs::create_dir_all(&fw_src_dir).unwrap();
        std::fs::create_dir_all(&mech_src_dir).unwrap();

        // Escribir archivos de inicio
        std::fs::write(fw_src_dir.join("main.cpp"), "void setup() {}").unwrap();
        std::fs::write(mech_src_dir.join("enclosure.step"), "STEP DATA V1").unwrap();

        // 3. Vincular los módulos
        run_link(p_path.clone(), "firmware", fw_src_dir.clone()).unwrap();
        run_link(p_path.clone(), "mechanical", mech_src_dir.clone()).unwrap();

        // 4. Primer commit modular
        let commit1 = run_commit(p_path.clone(), Some("Modular init".to_string())).unwrap();
        assert_eq!(commit1.message, "Modular init");
        assert!(commit1.modules.contains_key("firmware"));
        assert!(commit1.modules.contains_key("mechanical"));

        // 5. Modificar archivos
        std::fs::write(fw_src_dir.join("main.cpp"), "void setup() { /* Modificado */ }").unwrap();
        std::fs::write(mech_src_dir.join("enclosure.step"), "STEP DATA V2").unwrap();

        // Segundo commit modular
        let commit2 = run_commit(p_path.clone(), Some("Modular update".to_string())).unwrap();
        assert_eq!(commit2.message, "Modular update");

        // 6. Restaurar al primer commit
        let restored = run_restore(p_path.clone(), &commit1.hash[..8]).unwrap();
        assert!(restored.contains(&"firmware".to_string()));
        assert!(restored.contains(&"mechanical".to_string()));

        // Verificar restauración de archivos
        let fw_content = std::fs::read_to_string(fw_src_dir.join("main.cpp")).unwrap();
        let mech_content = std::fs::read_to_string(mech_src_dir.join("enclosure.step")).unwrap();
        assert_eq!(fw_content, "void setup() {}");
        assert_eq!(mech_content, "STEP DATA V1");

        // Limpiar
        std::fs::remove_dir_all(&temp_dir).ok();
    }
}
