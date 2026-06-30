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

    // 1. Localizar archivos originales de hardware en la carpeta del proyecto
    let mut cad_file_path = None;
    let mut bom_file_path = None;
    
    if let Ok(entries) = std::fs::read_dir(&project_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() {
                if let Some(ext) = path.extension().and_then(|s| s.to_str()) {
                    let ext_lower = ext.to_lowercase();
                    if ext_lower == "kicad_pcb" || ext_lower == "kicad_sch" || ext_lower == "brd" || ext_lower == "edif" || ext_lower == "edf" || (ext_lower == "sch" && !path.to_string_lossy().contains("bom")) {
                        cad_file_path = Some(path);
                    } else if ext_lower == "xlsx" || ext_lower == "xls" || (ext_lower == "csv" && path.file_name().and_then(|s| s.to_str()).unwrap_or("").to_lowercase().contains("bom")) {
                        bom_file_path = Some(path);
                    }
                }
            }
        }
    }

    let cad_path = cad_file_path.unwrap_or_else(|| project_dir.join("design.json"));
    if !cad_path.exists() {
        return Err("No se encontró ningún archivo de diseño de hardware (design.json, .kicad_pcb, .sch, .brd, .edif)".to_string());
    }

    let bom_path = bom_file_path.unwrap_or_else(|| project_dir.join("bom.csv"));

    let cad_bytes = std::fs::read(&cad_path)
        .map_err(|e| format!("Error al leer el archivo de diseño {}: {}", cad_path.display(), e))?;
        
    let bom_bytes = if bom_path.exists() {
        Some(std::fs::read(&bom_path).map_err(|e| format!("Error al leer el archivo BOM {}: {}", bom_path.display(), e))?)
    } else {
        None
    };

    let cad_filename = cad_path.file_name().and_then(|s| s.to_str()).unwrap_or("design.json").to_string();
    let bom_filename = if bom_bytes.is_some() {
        Some(bom_path.file_name().and_then(|s| s.to_str()).unwrap_or("bom.csv").to_string())
    } else {
        None
    };

    // 2. Calcular hash SHA-256 de los archivos originales
    let mut hasher = Sha256::new();
    hasher.update(&cad_bytes);
    if let Some(ref b_bytes) = bom_bytes {
        hasher.update(b_bytes);
    }
    
    let hash_result = hasher.finalize();
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

    // Si el hash coincide con el último commit, no hay cambios
    if hash_str == parent_hash {
        return Err("No hay cambios pendientes de hardware para respaldar ni sincronizar.".to_string());
    }

    // 4. Calcular diferencia semántica (diff) comparando con la caché actual (que representa el parent_hash)
    let cache_dir = project_dir.join(".ito").join("cache");
    let old_design = if cache_dir.exists() {
        parsers::parse_project_directory(&cache_dir).unwrap_or_else(|_| models::HardwareDesign::new())
    } else {
        models::HardwareDesign::new()
    };

    let new_design = parsers::parse_project_directory(&project_dir)
        .unwrap_or_else(|_| models::HardwareDesign::new());

    let diff_result = diff::diff_designs(&old_design, &new_design);
    let diff_summary = DiffSummary {
        added_components: diff_result.components.added.len(),
        deleted_components: diff_result.components.deleted.len(),
        modified_components: diff_result.components.modified.len(),
        added_nets: diff_result.nets.added.len(),
        deleted_nets: diff_result.nets.deleted.len(),
        modified_nets: diff_result.nets.modified.len(),
    };

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

    // Adjuntar archivo CAD original al ZIP
    zip.start_file(&cad_filename, options)
        .map_err(|e| format!("Error al añadir {} al zip: {}", cad_filename, e))?;
    zip.write_all(&cad_bytes)
        .map_err(|e| format!("Error al escribir {} al zip: {}", cad_filename, e))?;

    // Adjuntar archivo BOM original al ZIP (si existe)
    if let (Some(ref b_filename), Some(ref b_bytes)) = (&bom_filename, &bom_bytes) {
        zip.start_file(b_filename, options)
            .map_err(|e| format!("Error al añadir {} al zip: {}", b_filename, e))?;
        zip.write_all(b_bytes)
            .map_err(|e| format!("Error al escribir {} al zip: {}", b_filename, e))?;
    }
    zip.finish()
        .map_err(|e| format!("Error al finalizar zip: {}", e))?;

    // 6. Registrar en el historial local
    let timestamp = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let commit_msg = message.clone().unwrap_or_else(|| "Respaldo local de hardware".to_string());
    let relative_zip_path = format!(".ito/backups/{}.zip", hash_str);

    let commit_entry = CommitEntry {
        hash: hash_str.clone(),
        parent_hash: parent_hash.clone(),
        message: commit_msg.clone(),
        timestamp,
        zip_path: relative_zip_path,
        synced: true,
        diff_summary: Some(diff_summary),
    };

    history.commits.push(commit_entry.clone());
    let history_str = toml::to_string_pretty(&history)
        .map_err(|e| format!("Error al serializar historial: {}", e))?;
    std::fs::write(&history_path, history_str)
        .map_err(|e| format!("Error al escribir historial: {}", e))?;

    // 7. Actualizar la caché local para 'ito diff'
    if cache_dir.exists() {
        std::fs::remove_dir_all(&cache_dir).ok();
    }
    std::fs::create_dir_all(&cache_dir).ok();
    
    std::fs::write(cache_dir.join(&cad_filename), &cad_bytes).ok();
    if let (Some(ref b_filename), Some(ref b_bytes)) = (&bom_filename, &bom_bytes) {
        std::fs::write(cache_dir.join(b_filename), b_bytes).ok();
    }

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

    // 3. Abrir el archivo zip
    let absolute_zip_path = project_dir.join(&matched_commit.zip_path);
    if !absolute_zip_path.exists() {
        return Err(format!("No se encontró el archivo de respaldo en la ruta: {}", absolute_zip_path.display()));
    }

    let file = std::fs::File::open(&absolute_zip_path)
        .map_err(|e| format!("Error al abrir archivo de respaldo ZIP: {}", e))?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|e| format!("Error al leer el archivo ZIP: {}", e))?;

    // 4. Limpiar los archivos CAD/BOM activos antes de extraer los nuevos
    let mut active_cad_path = None;
    let mut active_bom_path = None;
    if let Ok(entries) = std::fs::read_dir(&project_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() {
                if let Some(ext) = path.extension().and_then(|s| s.to_str()) {
                    let ext_lower = ext.to_lowercase();
                    if ext_lower == "kicad_pcb" || ext_lower == "kicad_sch" || ext_lower == "brd" || ext_lower == "edif" || ext_lower == "edf" || (ext_lower == "sch" && !path.to_string_lossy().contains("bom")) {
                        active_cad_path = Some(path);
                    } else if ext_lower == "xlsx" || ext_lower == "xls" || (ext_lower == "csv" && path.file_name().and_then(|s| s.to_str()).unwrap_or("").to_lowercase().contains("bom")) {
                        active_bom_path = Some(path);
                    }
                }
            }
        }
    }

    if let Some(path) = active_cad_path {
        std::fs::remove_file(path).ok();
    }
    if let Some(path) = active_bom_path {
        std::fs::remove_file(path).ok();
    }

    // 5. Extraer los archivos del ZIP al directorio raíz del proyecto
    let mut restored_files = Vec::new();
    let cache_dir = project_dir.join(".ito").join("cache");
    if cache_dir.exists() {
        std::fs::remove_dir_all(&cache_dir).ok();
    }
    std::fs::create_dir_all(&cache_dir).ok();

    for i in 0..archive.len() {
        let mut file = archive.by_index(i)
            .map_err(|e| format!("Error al acceder al archivo dentro del ZIP: {}", e))?;
        let outpath = match file.enclosed_name() {
            Some(path) => path.to_owned(),
            None => continue,
        };

        let file_name = outpath.file_name().unwrap().to_str().unwrap().to_string();
        let target_dest_path = project_dir.join(&file_name);

        let mut outfile = std::fs::File::create(&target_dest_path)
            .map_err(|e| format!("Error al crear archivo de restauración: {}", e))?;
        std::io::copy(&mut file, &mut outfile)
            .map_err(|e| format!("Error al extraer archivo de restauración: {}", e))?;

        // También copiar a la caché para mantener ito diff alineado
        let cache_dest_path = cache_dir.join(&file_name);
        std::fs::copy(&target_dest_path, &cache_dest_path).ok();

        restored_files.push(file_name);
    }

    Ok(restored_files)
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
        let path = if sub_dir.is_empty() {
            project_dir.clone()
        } else {
            project_dir.join(sub_dir)
        };
        std::fs::create_dir_all(&path)
            .map_err(|e| format!("Error al crear el directorio '{}': {}", path.display(), e))?;
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
    let mut current = start_dir.to_path_buf();
    loop {
        if current.join("ito.json").is_file() || current.join(".ito").join("config.toml").is_file() {
            return Some(current);
        }
        if !current.pop() {
            break;
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

    // Crear enlace
    let link = models::ItoProjectLink {
        path: target_path.to_string_lossy().to_string(),
        tool: tool_detected.clone(),
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

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(restored[0], "design.json");

        // 5. Leer el archivo restaurado en disco y verificar que contiene R1 (no R2)
        let content = std::fs::read_to_string(&cad_path).unwrap();
        assert!(content.contains("R1"));
        assert!(!content.contains("R2"));

        // Limpiar
        std::fs::remove_dir_all(&temp_dir).ok();
    }
}
