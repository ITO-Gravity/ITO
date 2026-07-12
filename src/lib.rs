pub mod models;
pub mod parsers;
pub mod diff;
pub mod linter;
pub mod engines;
pub mod ignore;
pub mod cas;
pub mod updater;

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
                            let raw_path = std::path::PathBuf::from(&link.path);
                            let resolved_path = if raw_path.is_absolute() {
                                raw_path
                            } else {
                                project_dir.join(raw_path)
                            };
                            active_modules.push((module_name.to_string(), resolved_path, link.engine.clone()));
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
                let raw_path = std::path::PathBuf::from(&link.path);
                if raw_path.is_absolute() {
                    raw_path
                } else {
                    project_dir.join(raw_path)
                }
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

    Ok((project_dir, project_uuid))
}


fn load_manifest(project_dir: &std::path::Path) -> std::collections::HashSet<String> {
    let manifest_path = project_dir.join(".ito").join("manifest.json");
    if manifest_path.exists() {
        if let Ok(content) = std::fs::read_to_string(&manifest_path) {
            if let Ok(manifest) = serde_json::from_str::<std::collections::HashSet<String>>(&content) {
                return manifest;
            }
        }
    }
    std::collections::HashSet::new()
}

fn save_manifest(project_dir: &std::path::Path, manifest: &std::collections::HashSet<String>) {
    let manifest_path = project_dir.join(".ito").join("manifest.json");
    if let Ok(content) = serde_json::to_string_pretty(manifest) {
        let _ = std::fs::write(&manifest_path, content);
    }
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
    let existing_token = load_workspace_config().ok().flatten().and_then(|c| c.token);
    let config = models::ItoWorkspaceConfig {
        workspace: workspace_path.to_string_lossy().to_string(),
        version: "1.0".to_string(),
        token: existing_token,
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
                                let raw_path = std::path::PathBuf::from(&link.path);
                                let resolved_path = if raw_path.is_absolute() {
                                    raw_path
                                } else {
                                    project.path.join(raw_path)
                                };
                                let link_str = resolved_path.to_string_lossy().to_lowercase().replace('\\', "/");
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
                            let raw_path = std::path::PathBuf::from(&link.path);
                            let resolved_path = if raw_path.is_absolute() {
                                raw_path
                            } else {
                                project_root.join(raw_path)
                            };
                            let link_str = resolved_path.to_string_lossy().to_lowercase().replace('\\', "/");
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
    let resolved_target = if target_path.is_absolute() {
        target_path.clone()
    } else {
        project_root.join(&target_path)
    };

    if !resolved_target.is_dir() {
        return Err(format!("La ruta especificada '{}' no es un directorio válido o no existe.", resolved_target.display()));
    }

    let ito_json_path = project_root.join("ito.json");
    let mut config = if !ito_json_path.exists() {
        let project_name = project_root
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("ito-project")
            .to_string();
        let project_uuid = uuid::Uuid::new_v4().to_string();
        let created_at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let created_by = std::env::var("USER")
            .or_else(|_| std::env::var("USERNAME"))
            .unwrap_or_else(|_| "unknown".to_string());
        models::ItoProjectConfig {
            format_version: "1.0".to_string(),
            project_name,
            project_uuid,
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
        }
    } else {
        let content = std::fs::read_to_string(&ito_json_path)
            .map_err(|e| format!("Error al leer ito.json: {}", e))?;
        serde_json::from_str(&content)
            .map_err(|e| format!("Error al parsear ito.json: {}", e))?
    };

    // Detectar herramienta
    let tool_detected = detect_tool_in_path(&resolved_target);

    // Detectar motor por defecto según el módulo y herramientas presentes
    let engine_detected = match module_key {
        "firmware" => {
            if resolved_target.join(".git").is_dir() {
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
        "[Console]::OutputEncoding = [System.Text.Encoding]::UTF8; \
         Add-Type -AssemblyName System.Windows.Forms; \
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
    // Limpiar ejecutables temporales o antiguos generados por el auto-actualizador
    updater::cleanup_old_executable();

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

pub fn resolve_token(config: &Config) -> std::result::Result<String, String> {
    if let Some(ref t) = config.token {
        if !t.trim().is_empty() {
            return Ok(t.clone());
        }
    }
    // Fallback al token global
    if let Ok(Some(ws_cfg)) = load_workspace_config() {
        if let Some(ref t) = ws_cfg.token {
            if !t.trim().is_empty() {
                return Ok(t.clone());
            }
        }
    }
    Err("No estás autenticado. Por favor inicia sesión con: ito login".to_string())
}

pub fn save_global_workspace_config(config: &models::ItoWorkspaceConfig) -> std::result::Result<(), String> {
    let config_str = serde_json::to_string_pretty(config)
        .map_err(|e| format!("Error al serializar configuración: {}", e))?;

    // Guardar en ~/.ito/config.json
    let pointer_path = get_global_config_pointer_path()?;
    if let Some(parent) = pointer_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Error al crear directorio global de configuración {}: {}", parent.display(), e))?;
    }
    std::fs::write(&pointer_path, &config_str)
        .map_err(|e| format!("Error al escribir puntero de configuración en {}: {}", pointer_path.display(), e))?;

    // Guardar en Workspace/Config/config.json si existe
    let workspace_path = std::path::PathBuf::from(&config.workspace);
    let local_config_path = workspace_path.join("Config").join("config.json");
    if local_config_path.parent().map(|p| p.exists()).unwrap_or(false) {
        let _ = std::fs::write(&local_config_path, &config_str);
    }

    Ok(())
}

pub async fn run_login(email: &str, password: &str) -> std::result::Result<String, String> {
    let remote_url = "https://itogravity.com/php/ito_api.php".to_string();

    let client = reqwest::Client::new();
    let mut params = std::collections::HashMap::new();
    params.insert("action", "login");
    params.insert("email", email);
    params.insert("password", password);

    println!("Conectando con el servidor para iniciar sesión...");
    let response = client.post(&remote_url)
        .form(&params)
        .send()
        .await
        .map_err(|e| format!("Error de conexión al servidor: {}", e))?;

    if !response.status().is_success() {
        // Intentar decodificar mensaje de error del servidor
        if let Ok(resp_json) = response.json::<serde_json::Value>().await {
            if let Some(msg) = resp_json.get("message").and_then(|m| m.as_str()) {
                return Err(msg.to_string());
            }
        }
        return Err("Inicio de sesión fallido. Verifica tus credenciales.".to_string());
    }

    let resp_json: serde_json::Value = response.json()
        .await
        .map_err(|e| format!("Error al decodificar respuesta del servidor: {}", e))?;

    let token = resp_json.get("token")
        .and_then(|t| t.as_str())
        .ok_or_else(|| "El servidor no retornó un token de sesión.".to_string())?;

    let operator_name = resp_json.get("operator_name")
        .and_then(|o| o.as_str())
        .unwrap_or("Operador");

    // Guardar token en la configuración global
    let mut ws_config = match load_workspace_config()? {
        Some(cfg) => cfg,
        None => {
            let default_ws = get_default_workspace_path()?;
            models::ItoWorkspaceConfig {
                workspace: default_ws.to_string_lossy().to_string(),
                version: "1.0".to_string(),
                token: None,
            }
        }
    };

    ws_config.token = Some(token.to_string());
    save_global_workspace_config(&ws_config)?;

    Ok(format!("Sesión iniciada con éxito. ¡Bienvenido, {}!", operator_name))
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
                        let raw_path = std::path::PathBuf::from(&link.path);
                        let path = if raw_path.is_absolute() {
                            raw_path
                        } else {
                            project_dir.join(raw_path)
                        };
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

        let mut tracked_files = std::collections::HashSet::new();

        fn walk_and_zip_dir(
            dir: &std::path::Path,
            base_dir: &std::path::Path,
            prefix_in_zip: &str,
            filter: &ignore::IgnoreFilter,
            zip: &mut zip::ZipWriter<std::io::Cursor<&mut Vec<u8>>>,
            options: zip::write::FileOptions,
            links: &std::collections::HashMap<String, models::ItoProjectLink>,
            is_root_walk: bool,
            tracked_files: &mut std::collections::HashSet<String>,
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

                        walk_and_zip_dir(&path, base_dir, prefix_in_zip, filter, zip, options, links, is_root_walk, tracked_files)?;
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
                        
                        tracked_files.insert(file_name_in_zip);
                    }
                }
            }
            Ok(())
        }

        walk_and_zip_dir(project_dir, project_dir, "", &filter, &mut zip, options, &links, true, &mut tracked_files)?;

        for (module_name, link) in &links {
            let raw_path = std::path::Path::new(&link.path);
            let external_path = if raw_path.is_absolute() {
                raw_path.to_path_buf()
            } else {
                project_dir.join(raw_path)
            };

            if external_path.exists() && external_path.is_dir() {
                let ext_filter = ignore::IgnoreFilter::new(&external_path);
                walk_and_zip_dir(&external_path, &external_path, module_name, &ext_filter, &mut zip, options, &links, false, &mut tracked_files)?;
            }
        }

        zip.finish()
            .map_err(|e| format!("Error al finalizar archivo zip: {}", e))?;

        save_manifest(project_dir, &tracked_files);
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

    let token = resolve_token(&config)?;

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

    let token = resolve_token(&config)?;

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

    let old_manifest = load_manifest(&project_dir);
    let mut new_manifest = std::collections::HashSet::new();

    for i in 0..archive.len() {
        let mut file = archive.by_index(i)
            .map_err(|e| format!("Error al leer entrada del ZIP: {}", e))?;
        let outpath = match file.enclosed_name() {
            Some(path) => project_dir.join(path),
            None => continue,
        };

        let file_name = file.name().to_string();
        if file_name.ends_with('/') {
            std::fs::create_dir_all(&outpath).ok();
        } else {
            if let Some(p) = outpath.parent() {
                std::fs::create_dir_all(p).ok();
            }
            let mut outfile = std::fs::File::create(&outpath)
                .map_err(|e| format!("Error al crear archivo local: {}", e))?;
            std::io::copy(&mut file, &mut outfile)
                .map_err(|e| format!("Error al escribir archivo: {}", e))?;
            
            let rel_path = file_name.replace('\\', "/");
            new_manifest.insert(rel_path);
        }
    }

    for rel_path in &old_manifest {
        if !new_manifest.contains(rel_path) {
            let file_to_delete = project_dir.join(rel_path);
            if file_to_delete.exists() && file_to_delete.is_file() {
                let _ = std::fs::remove_file(&file_to_delete);
                println!("Eliminado archivo local obsoleto: {}", rel_path);
            }
        }
    }

    // Caso especial de transición para versión 0.3.3: Si LICENSE existe localmente pero no en el ZIP (servidor), lo removemos
    let license_file = project_dir.join("LICENSE");
    if !new_manifest.contains("LICENSE") && license_file.exists() && license_file.is_file() {
        let _ = std::fs::remove_file(&license_file);
        println!("Eliminado archivo local obsoleto: LICENSE");
    }

    save_manifest(&project_dir, &new_manifest);

    let mut electronics_path = project_dir.clone();
    let ito_json_path = project_dir.join("ito.json");
    if ito_json_path.exists() {
        if let Ok(c) = std::fs::read_to_string(&ito_json_path) {
            if let Ok(cfg) = serde_json::from_str::<models::ItoProjectConfig>(&c) {
                if let Some(links) = cfg.links {
                    if let Some(link) = links.get("electronics") {
                        let raw_path = std::path::PathBuf::from(&link.path);
                        electronics_path = if raw_path.is_absolute() {
                            raw_path
                        } else {
                            project_dir.join(raw_path)
                        };
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

pub async fn run_clone(token_or_project: String) -> std::result::Result<String, String> {
    // 1. Resolver token y project_id_input
    let (token, project_id_input) = if token_or_project.starts_with("ito_tk_") {
        // Es un token específico de proyecto (retrocompatibilidad)
        (token_or_project.clone(), None)
    } else {
        // Es un nombre/ID/URL de proyecto. Necesitamos el token global
        let global_token = match load_workspace_config() {
            Ok(Some(cfg)) => cfg.token.ok_or_else(|| "No estás autenticado. Ejecutá: ito login".to_string())?,
            _ => return Err("No estás autenticado. Ejecutá: ito login".to_string()),
        };
        (global_token, Some(token_or_project.clone()))
    };

    let clean_project_id = project_id_input.map(|input| {
        let input_trimmed = input.trim();
        if input_trimmed.starts_with("http://") || input_trimmed.starts_with("https://") {
            if let Some(pos) = input_trimmed.rfind('/') {
                input_trimmed[pos + 1..].to_string()
            } else {
                input_trimmed.to_string()
            }
        } else {
            input_trimmed.to_string()
        }
    });

    let remote_url = "https://itogravity.com/php/ito_api.php".to_string();

    let client = reqwest::Client::new();
    let mut params = std::collections::HashMap::new();
    params.insert("action", "info");
    params.insert("token", &token);
    if let Some(ref pid) = clean_project_id {
        params.insert("project_id", pid);
    }

    println!("Conectando con el servidor para verificar el token...");
    let response = client.post(&remote_url)
        .form(&params)
        .send()
        .await
        .map_err(|e| format!("Error de conexión al servidor: {}", e))?;

    let mut project_id_to_use = None;
    let mut project_name_to_use = None;

    if response.status().is_success() {
        if let Ok(resp_json) = response.json::<serde_json::Value>().await {
            if let Some(proj_id) = resp_json.get("project_id") {
                let id_str = match proj_id {
                    serde_json::Value::Number(n) => n.to_string(),
                    serde_json::Value::String(s) => s.clone(),
                    _ => "".to_string(),
                };
                if !id_str.is_empty() {
                    project_id_to_use = Some(id_str);
                }
            }
            if let Some(proj_name) = resp_json.get("project_name").and_then(|n| n.as_str()) {
                project_name_to_use = Some(proj_name.to_string());
            }
        }
    }

    if project_name_to_use.is_none() {
        // Fallback: intentar verificar con action=latest (soportado por servidores viejos)
        let mut fallback_params = std::collections::HashMap::new();
        fallback_params.insert("action", "latest");
        fallback_params.insert("token", &token);

        let fb_response = client.post(&remote_url)
            .form(&fallback_params)
            .send()
            .await
            .map_err(|e| format!("Error de conexión al servidor (fallback): {}", e))?;

        let status_code = fb_response.status().as_u16();
        if status_code == 401 {
            return Err("Token inválido o expirado. Verifica tus credenciales.".to_string());
        }

        if fb_response.status().is_success() || status_code == 404 {
            println!("Servidor no actualizado detected. Token verificado con éxito.");
            println!("Ingresa el nombre del proyecto en la web (ej. PRUEBA-ITO):");
            let mut input = String::new();
            std::io::stdin().read_line(&mut input)
                .map_err(|e| format!("Error al leer entrada: {}", e))?;
            let name_trimmed = input.trim().to_string();
            if name_trimmed.is_empty() {
                return Err("El nombre del proyecto no puede estar vacío.".to_string());
            }
            project_id_to_use = Some(name_trimmed.clone());
            project_name_to_use = Some(name_trimmed);
        } else {
            return Err("Token inválido o expirado. Verifica tus credenciales.".to_string());
        }
    }

    let id_str = project_id_to_use.ok_or_else(|| "No se pudo obtener el ID del proyecto.".to_string())?;
    let project_name = project_name_to_use.ok_or_else(|| "No se pudo obtener el nombre del proyecto.".to_string())?;

    let target_dir = std::env::current_dir()
        .map_err(|e| format!("Error al obtener el directorio actual: {}", e))?
        .join(&project_name);

    if target_dir.exists() {
        return Err(format!("Error: El directorio '{}' ya existe.", target_dir.display()));
    }

    println!("Creando directorio del proyecto '{}'...", project_name);
    std::fs::create_dir_all(&target_dir)
        .map_err(|e| format!("Error al crear el directorio del proyecto: {}", e))?;

    let ito_dir = target_dir.join(".ito");
    std::fs::create_dir_all(&ito_dir)
        .map_err(|e| format!("Error al crear el directorio .ito: {}", e))?;

    // Crear config.toml
    let config = Config {
        project_id: id_str.clone(),
        remote_url: remote_url.clone(),
        token: if token_or_project.starts_with("ito_tk_") {
            Some(token.clone())
        } else {
            None
        },
    };
    let toml_str = toml::to_string_pretty(&config)
        .map_err(|e| format!("Error al serializar configuración: {}", e))?;
    std::fs::write(ito_dir.join("config.toml"), toml_str)
        .map_err(|e| format!("Error al escribir configuración: {}", e))?;

    // Crear el ito.json por defecto
    let ito_json_path = target_dir.join("ito.json");
    let ito_config = models::ItoProjectConfig {
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
    };
    let c_json = serde_json::to_string_pretty(&ito_config)
        .map_err(|e| format!("Error al serializar ito.json: {}", e))?;
    std::fs::write(&ito_json_path, c_json)
        .map_err(|e| format!("Error al escribir ito.json: {}", e))?;

    println!("Descargando versión completa desde el servidor...");
    // Ejecutar run_pull
    match run_pull(target_dir.clone()).await {
        Ok(msg) => {
            Ok(format!("Proyecto '{}' clonado con éxito en: {}\n{}", project_name, target_dir.display(), msg))
        }
        Err(e) => {
            // Limpiar carpeta en caso de error para no dejar residuos
            let _ = std::fs::remove_dir_all(&target_dir);
            Err(format!("Error al descargar archivos del proyecto: {}", e))
        }
    }
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

    #[test]
    fn test_zip_with_relative_link() {
        let unique_id = uuid::Uuid::new_v4().to_string();
        let temp_dir = std::env::temp_dir().join(format!("ito-rel-zip-{}", unique_id));
        std::fs::create_dir_all(&temp_dir).unwrap();

        // 1. Crear proyecto
        let (p_path, _) = run_new(temp_dir.clone(), "MyProj").unwrap();

        // 2. Crear una carpeta externa simulada
        let fw_path = temp_dir.join("MyExternalFW");
        std::fs::create_dir_all(&fw_path).unwrap();
        std::fs::write(fw_path.join("main.cpp"), "void main() {}").unwrap();

        // 3. Vincular usando ruta relativa (relativa a p_path)
        let relative_target = std::path::PathBuf::from("../MyExternalFW");
        run_link(p_path.clone(), "firmware", relative_target).unwrap();

        // 4. Crear ZIP del proyecto
        let zip_bytes = create_project_zip(&p_path).unwrap();
        assert!(!zip_bytes.is_empty());

        // 5. Leer el ZIP y verificar que firmware/main.cpp existe en él
        let reader = std::io::Cursor::new(zip_bytes);
        let mut zip = zip::ZipArchive::new(reader).unwrap();
        
        let mut found_main_cpp = false;
        let mut content_matched = false;
        for i in 0..zip.len() {
            let mut file = zip.by_index(i).unwrap();
            if file.name() == "firmware/main.cpp" {
                found_main_cpp = true;
                let mut buf = Vec::new();
                use std::io::Read;
                file.read_to_end(&mut buf).unwrap();
                if String::from_utf8(buf).unwrap() == "void main() {}" {
                    content_matched = true;
                }
                break;
            }
        }
        assert!(found_main_cpp, "No se encontró el archivo firmware/main.cpp en el archivo zip empaquetado");
        assert!(content_matched, "El contenido del archivo en el zip no coincide o está vacío");

        // Limpiar
        std::fs::remove_dir_all(&temp_dir).ok();
    }

    #[test]
    fn test_manifest_file_deletion_tracking() {
        let unique_id = uuid::Uuid::new_v4().to_string();
        let temp_dir = std::env::temp_dir().join(format!("ito-test-manifest-{}", unique_id));
        std::fs::create_dir_all(&temp_dir).unwrap();

        // 1. Crear la subcarpeta .ito para simular el proyecto
        std::fs::create_dir_all(temp_dir.join(".ito")).unwrap();

        // 2. Crear archivos simulados en el directorio del proyecto
        let file1_path = temp_dir.join("file1.txt");
        let file2_path = temp_dir.join("file2.txt");
        std::fs::write(&file1_path, "Content 1").unwrap();
        std::fs::write(&file2_path, "Content 2").unwrap();

        assert!(file1_path.exists());
        assert!(file2_path.exists());

        // 3. Guardar el manifiesto con ambos archivos
        let mut old_manifest = std::collections::HashSet::new();
        old_manifest.insert("file1.txt".to_string());
        old_manifest.insert("file2.txt".to_string());
        save_manifest(&temp_dir, &old_manifest);

        // 4. Verificar que se cargue correctamente
        let loaded = load_manifest(&temp_dir);
        assert_eq!(loaded.len(), 2);
        assert!(loaded.contains("file1.txt"));
        assert!(loaded.contains("file2.txt"));

        // 5. Simular la extracción de un ZIP que contiene sólo file1.txt (eliminamos file2.txt del manifiesto)
        let mut new_manifest = std::collections::HashSet::new();
        new_manifest.insert("file1.txt".to_string());

        // Ejecutar lógica de limpieza (similar a la que está en run_pull)
        for rel_path in &loaded {
            if !new_manifest.contains(rel_path) {
                let file_to_delete = temp_dir.join(rel_path);
                if file_to_delete.exists() && file_to_delete.is_file() {
                    let _ = std::fs::remove_file(&file_to_delete);
                }
            }
        }

        // 6. Verificar que file2.txt haya sido eliminado y file1.txt siga existiendo
        assert!(file1_path.exists());
        assert!(!file2_path.exists());

        // Guardar el nuevo manifiesto y verificar
        save_manifest(&temp_dir, &new_manifest);
        let loaded_new = load_manifest(&temp_dir);
        assert_eq!(loaded_new.len(), 1);
        assert!(loaded_new.contains("file1.txt"));
        assert!(!loaded_new.contains("file2.txt"));

        // Limpiar
        std::fs::remove_dir_all(&temp_dir).ok();
    }
}

