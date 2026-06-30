// src/engines.rs

use std::path::Path;
use std::collections::HashMap;
use std::io::Write;
use crate::models::{HardwareDesign};
use crate::parsers;
use crate::diff;
use sha2::{Sha256, Digest};

#[derive(Debug, serde::Serialize, serde::Deserialize, Clone)]
pub enum ModuleStatus {
    Unchanged,
    Modified {
        summary: String,
        details: Vec<String>,
    },
    Error(String),
}

#[derive(Debug, serde::Serialize, serde::Deserialize, Clone)]
pub struct CommitPayload {
    pub engine_name: String,
    pub changes_detected: bool,
    pub details: Vec<String>,
    pub metadata: HashMap<String, String>,
}

pub trait Engine: Send + Sync {
    fn name(&self) -> &str;
    fn detect(&self, path: &Path) -> bool;
    fn status(&self, path: &Path, cache_dir: &Path) -> Result<ModuleStatus, String>;
    fn commit(&self, path: &Path, backup_dir: &Path, cache_dir: &Path) -> Result<CommitPayload, String>;
    fn restore(&self, path: &Path, backup_dir: &Path, payload: &CommitPayload) -> Result<(), String>;
}

// ----------------------------------------------------
// 1. GitEngine (Firmware)
// ----------------------------------------------------
pub struct GitEngine;

impl Engine for GitEngine {
    fn name(&self) -> &str {
        "git"
    }

    fn detect(&self, path: &Path) -> bool {
        path.join(".git").is_dir()
    }

    fn status(&self, path: &Path, _cache_dir: &Path) -> Result<ModuleStatus, String> {
        let output = std::process::Command::new("git")
            .args(&["status", "--porcelain"])
            .current_dir(path)
            .output();

        match output {
            Ok(out) => {
                let stdout = String::from_utf8_lossy(&out.stdout);
                let lines: Vec<&str> = stdout.lines().filter(|l| !l.is_empty()).collect();
                if lines.is_empty() {
                    Ok(ModuleStatus::Unchanged)
                } else {
                    let summary = format!("{} archivos modificados en Git", lines.len());
                    let details = lines.iter().map(|s| s.to_string()).collect();
                    Ok(ModuleStatus::Modified { summary, details })
                }
            }
            Err(e) => {
                Ok(ModuleStatus::Error(format!("Error al ejecutar git status: {}", e)))
            }
        }
    }

    fn commit(&self, path: &Path, _backup_dir: &Path, _cache_dir: &Path) -> Result<CommitPayload, String> {
        let output = std::process::Command::new("git")
            .args(&["rev-parse", "--short", "HEAD"])
            .current_dir(path)
            .output();

        let mut metadata = HashMap::new();
        let mut details = Vec::new();
        let mut changes_detected = false;

        if let Ok(out) = output {
            let hash = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !hash.is_empty() {
                metadata.insert("git_commit".to_string(), hash.clone());
                details.push(format!("Git commit: {}", hash));
            }
        }

        if let Ok(ModuleStatus::Modified { details: d, .. }) = self.status(path, _cache_dir) {
            changes_detected = true;
            details.extend(d);
        }

        Ok(CommitPayload {
            engine_name: self.name().to_string(),
            changes_detected,
            details,
            metadata,
        })
    }

    fn restore(&self, path: &Path, _backup_dir: &Path, payload: &CommitPayload) -> Result<(), String> {
        if let Some(git_commit) = payload.metadata.get("git_commit") {
            println!("Ejecutando git checkout {}...", git_commit);
            let _ = std::process::Command::new("git")
                .args(&["checkout", git_commit])
                .current_dir(path)
                .status();
        }
        Ok(())
    }
}

// ----------------------------------------------------
// 2. SemanticCadEngine (Electrónica)
// ----------------------------------------------------
pub struct SemanticCadEngine;

impl Engine for SemanticCadEngine {
    fn name(&self) -> &str {
        "semantic-cad"
    }

    fn detect(&self, path: &Path) -> bool {
        if let Ok(entries) = std::fs::read_dir(path) {
            for entry in entries.flatten() {
                if let Some(ext) = entry.path().extension().and_then(|s| s.to_str()) {
                    let ext_lower = ext.to_lowercase();
                    if ext_lower == "kicad_pcb" || ext_lower == "kicad_sch" || ext_lower == "brd" || ext_lower == "edif" || ext_lower == "edf" || ext_lower == "sch" || entry.file_name() == "design.json" {
                        return true;
                    }
                }
            }
        }
        false
    }

    fn status(&self, path: &Path, cache_dir: &Path) -> Result<ModuleStatus, String> {
        let new_design = parsers::parse_project_directory(path).unwrap_or_else(|_| HardwareDesign::new());
        let old_design = parsers::parse_project_directory(cache_dir).unwrap_or_else(|_| HardwareDesign::new());

        let diff_result = diff::diff_designs(&old_design, &new_design);

        if diff_result.is_empty() {
            Ok(ModuleStatus::Unchanged)
        } else {
            let mut details = Vec::new();
            for c in diff_result.components.added.values() {
                details.push(format!("+ Componente: {}", c.designator));
            }
            for c in diff_result.components.deleted.values() {
                details.push(format!("- Componente: {}", c.designator));
            }
            for d in diff_result.components.modified.keys() {
                details.push(format!("~ Componente: {}", d));
            }
            for n in diff_result.nets.added.keys() {
                details.push(format!("+ Net: {}", n));
            }
            for n in diff_result.nets.deleted.keys() {
                details.push(format!("- Net: {}", n));
            }
            for n in diff_result.nets.modified.keys() {
                details.push(format!("~ Net: {}", n));
            }

            let summary_str = format!("{} comp, {} nets modificados", 
                diff_result.components.added.len() + diff_result.components.deleted.len() + diff_result.components.modified.len(),
                diff_result.nets.added.len() + diff_result.nets.deleted.len() + diff_result.nets.modified.len()
            );

            Ok(ModuleStatus::Modified {
                summary: summary_str,
                details,
            })
        }
    }

    fn commit(&self, path: &Path, backup_dir: &Path, cache_dir: &Path) -> Result<CommitPayload, String> {
        let mut changes_detected = false;
        let mut details = Vec::new();
        let mut metadata = HashMap::new();

        let stat = self.status(path, cache_dir)?;
        if let ModuleStatus::Modified { summary, details: d } = stat {
            changes_detected = true;
            details.push(summary);
            details.extend(d);
        }

        std::fs::create_dir_all(backup_dir).ok();

        let zip_path = backup_dir.join("electronics.zip");
        let zip_file = std::fs::File::create(&zip_path)
            .map_err(|e| format!("Error al crear archivo zip: {}", e))?;
        let mut zip = zip::ZipWriter::new(zip_file);
        let options = zip::write::FileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);

        std::fs::create_dir_all(cache_dir).ok();

        if let Ok(entries) = std::fs::read_dir(path) {
            for entry in entries.flatten() {
                let file_path = entry.path();
                if file_path.is_file() {
                    let filename = entry.file_name().to_string_lossy().to_string();
                    let ext_lower = file_path.extension().and_then(|s| s.to_str()).unwrap_or("").to_lowercase();
                    let is_cad_or_bom = ext_lower == "kicad_pcb" || ext_lower == "kicad_sch" || ext_lower == "brd" || ext_lower == "edif" || ext_lower == "edf" || ext_lower == "sch" || filename == "design.json" || filename.to_lowercase().contains("bom");
                    
                    if is_cad_or_bom {
                        if let Ok(bytes) = std::fs::read(&file_path) {
                            zip.start_file(&filename, options)
                                .map_err(|e| format!("Error al añadir al zip: {}", e))?;
                            zip.write_all(&bytes).ok();
                            
                            std::fs::write(cache_dir.join(&filename), &bytes).ok();
                        }
                    }
                }
            }
        }
        zip.finish().ok();
        
        metadata.insert("zip_file".to_string(), "electronics.zip".to_string());

        Ok(CommitPayload {
            engine_name: self.name().to_string(),
            changes_detected,
            details,
            metadata,
        })
    }

    fn restore(&self, path: &Path, backup_dir: &Path, payload: &CommitPayload) -> Result<(), String> {
        let zip_filename = payload.metadata.get("zip_file").cloned().unwrap_or_else(|| "electronics.zip".to_string());
        let zip_path = backup_dir.join(zip_filename);
        if !zip_path.exists() {
            return Err(format!("No se encontró el archivo de respaldo: {}", zip_path.display()));
        }

        if let Ok(entries) = std::fs::read_dir(path) {
            for entry in entries.flatten() {
                let file_path = entry.path();
                if file_path.is_file() {
                    let filename = entry.file_name().to_string_lossy().to_string();
                    let ext_lower = file_path.extension().and_then(|s| s.to_str()).unwrap_or("").to_lowercase();
                    let is_cad_or_bom = ext_lower == "kicad_pcb" || ext_lower == "kicad_sch" || ext_lower == "brd" || ext_lower == "edif" || ext_lower == "edf" || ext_lower == "sch" || filename == "design.json" || filename.to_lowercase().contains("bom");
                    if is_cad_or_bom {
                        std::fs::remove_file(file_path).ok();
                    }
                }
            }
        }

        let file = std::fs::File::open(&zip_path)
            .map_err(|e| format!("Error al abrir ZIP: {}", e))?;
        let mut archive = zip::ZipArchive::new(file)
            .map_err(|e| format!("Error al leer ZIP: {}", e))?;

        for i in 0..archive.len() {
            let mut file = archive.by_index(i)
                .map_err(|e| format!("Error al leer archivo en ZIP: {}", e))?;
            let outpath = match file.enclosed_name() {
                Some(path) => path.to_owned(),
                None => continue,
            };
            let dest_path = path.join(outpath);
            if let Some(p) = dest_path.parent() {
                std::fs::create_dir_all(p).ok();
            }
            let mut outfile = std::fs::File::create(&dest_path)
                .map_err(|e| format!("Error al crear archivo de destino: {}", e))?;
            std::io::copy(&mut file, &mut outfile).ok();
        }

        Ok(())
    }
}

// ----------------------------------------------------
// 3. FileHashEngine (Mecánica, Documentación, Manufactura)
// ----------------------------------------------------
pub struct FileHashEngine;

impl FileHashEngine {
    fn scan_and_hash(&self, path: &Path) -> HashMap<String, String> {
        let mut hashes = HashMap::new();
        self.scan_and_hash_recursive(path, path, &mut hashes);
        hashes
    }

    fn scan_and_hash_recursive(&self, root: &Path, current: &Path, hashes: &mut HashMap<String, String>) {
        if let Ok(entries) = std::fs::read_dir(current) {
            for entry in entries.flatten() {
                let file_path = entry.path();
                let file_name = entry.file_name().to_string_lossy().to_string();
                if file_name.starts_with('.') || file_name == "target" || file_name == "node_modules" {
                    continue;
                }
                if file_path.is_dir() {
                    self.scan_and_hash_recursive(root, &file_path, hashes);
                } else if file_path.is_file() {
                    if let Ok(bytes) = std::fs::read(&file_path) {
                        let mut hasher = Sha256::new();
                        hasher.update(&bytes);
                        let hash = format!("{:x}", hasher.finalize());
                        if let Ok(relative) = file_path.strip_prefix(root) {
                            hashes.insert(relative.to_string_lossy().to_string().replace('\\', "/"), hash);
                        }
                    }
                }
            }
        }
    }
}

impl Engine for FileHashEngine {
    fn name(&self) -> &str {
        "file-hash"
    }

    fn detect(&self, _path: &Path) -> bool {
        true
    }

    fn status(&self, path: &Path, cache_dir: &Path) -> Result<ModuleStatus, String> {
        let current_hashes = self.scan_and_hash(path);
        
        let hash_json_path = cache_dir.join("hashes.json");
        let old_hashes: HashMap<String, String> = if hash_json_path.exists() {
            let content = std::fs::read_to_string(&hash_json_path).unwrap_or_default();
            serde_json::from_str(&content).unwrap_or_default()
        } else {
            HashMap::new()
        };

        let mut added = Vec::new();
        let mut modified = Vec::new();
        let mut deleted = Vec::new();

        for (file, hash) in &current_hashes {
            match old_hashes.get(file) {
                Some(old_hash) => {
                    if hash != old_hash {
                        modified.push(file.clone());
                    }
                }
                None => {
                    added.push(file.clone());
                }
            }
        }

        for file in old_hashes.keys() {
            if !current_hashes.contains_key(file) {
                deleted.push(file.clone());
            }
        }

        if added.is_empty() && modified.is_empty() && deleted.is_empty() {
            Ok(ModuleStatus::Unchanged)
        } else {
            let mut details = Vec::new();
            for f in &added {
                details.push(format!("+ {}", f));
            }
            for f in &modified {
                details.push(format!("~ {}", f));
            }
            for f in &deleted {
                details.push(format!("- {}", f));
            }

            let summary = format!("{} archivos modificados ({} creados, {} modificados, {} borrados)", 
                added.len() + modified.len() + deleted.len(),
                added.len(), modified.len(), deleted.len()
            );

            Ok(ModuleStatus::Modified { summary, details })
        }
    }

    fn commit(&self, path: &Path, backup_dir: &Path, cache_dir: &Path) -> Result<CommitPayload, String> {
        let mut changes_detected = false;
        let mut details = Vec::new();
        let mut metadata = HashMap::new();

        let stat = self.status(path, cache_dir)?;
        if let ModuleStatus::Modified { summary, details: d } = stat {
            changes_detected = true;
            details.push(summary);
            details.extend(d);
        }

        std::fs::create_dir_all(backup_dir).ok();
        let zip_filename = "files.zip";
        let zip_path = backup_dir.join(zip_filename);
        let zip_file = std::fs::File::create(&zip_path)
            .map_err(|e| format!("Error al crear zip: {}", e))?;
        let mut zip = zip::ZipWriter::new(zip_file);
        let options = zip::write::FileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);

        std::fs::create_dir_all(cache_dir).ok();
        let current_hashes = self.scan_and_hash(path);
        let hash_json_path = cache_dir.join("hashes.json");
        let hash_json_content = serde_json::to_string_pretty(&current_hashes).unwrap_or_default();
        std::fs::write(&hash_json_path, hash_json_content).ok();

        for file_rel in current_hashes.keys() {
            let file_abs = path.join(file_rel);
            if file_abs.is_file() {
                if let Ok(bytes) = std::fs::read(&file_abs) {
                    zip.start_file(file_rel, options).ok();
                    zip.write_all(&bytes).ok();
                }
            }
        }
        zip.finish().ok();

        metadata.insert("zip_file".to_string(), zip_filename.to_string());

        Ok(CommitPayload {
            engine_name: self.name().to_string(),
            changes_detected,
            details,
            metadata,
        })
    }

    fn restore(&self, path: &Path, backup_dir: &Path, payload: &CommitPayload) -> Result<(), String> {
        let zip_filename = payload.metadata.get("zip_file").cloned().unwrap_or_else(|| "files.zip".to_string());
        let zip_path = backup_dir.join(zip_filename);
        if !zip_path.exists() {
            return Err(format!("No se encontró el archivo de respaldo: {}", zip_path.display()));
        }

        let file = std::fs::File::open(&zip_path)
            .map_err(|e| format!("Error al abrir ZIP: {}", e))?;
        let mut archive = zip::ZipArchive::new(file)
            .map_err(|e| format!("Error al leer ZIP: {}", e))?;

        for i in 0..archive.len() {
            let mut file = archive.by_index(i)
                .map_err(|e| format!("Error al leer archivo en ZIP: {}", e))?;
            let outpath = match file.enclosed_name() {
                Some(path) => path.to_owned(),
                None => continue,
            };
            let dest_path = path.join(outpath);
            if let Some(p) = dest_path.parent() {
                std::fs::create_dir_all(p).ok();
            }
            let mut outfile = std::fs::File::create(&dest_path)
                .map_err(|e| format!("Error al crear archivo de destino: {}", e))?;
            std::io::copy(&mut file, &mut outfile).ok();
        }

        Ok(())
    }
}

// ----------------------------------------------------
// 4. Registro y Fábrica de Motores
// ----------------------------------------------------
pub struct EngineRegistry {
    engines: Vec<Box<dyn Engine>>,
}

impl EngineRegistry {
    pub fn new() -> Self {
        let mut registry = Self { engines: Vec::new() };
        registry.register(Box::new(GitEngine));
        registry.register(Box::new(SemanticCadEngine));
        registry.register(Box::new(FileHashEngine));
        registry
    }

    pub fn register(&mut self, engine: Box<dyn Engine>) {
        self.engines.push(engine);
    }

    pub fn get_engine(&self, name: &str) -> Option<&dyn Engine> {
        self.engines.iter()
            .find(|e| e.name() == name)
            .map(|e| e.as_ref())
    }
}