// src/engines.rs

use std::path::{Path, PathBuf};
use std::collections::HashMap;
use crate::models::{HardwareDesign};
use crate::ignore::IgnoreFilter;
use crate::cas;
use crate::parsers;
use crate::diff;

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
// Auxiliar: Escaneo recursivo respetando ignores
// ----------------------------------------------------
fn scan_directory_recursive(
    root: &Path,
    current: &Path,
    filter: &IgnoreFilter,
    files: &mut Vec<PathBuf>,
) {
    if filter.is_ignored(current) {
        return;
    }
    if let Ok(entries) = std::fs::read_dir(current) {
        for entry in entries.flatten() {
            let path = entry.path();
            if filter.is_ignored(&path) {
                continue;
            }
            if path.is_dir() {
                scan_directory_recursive(root, &path, filter, files);
            } else if path.is_file() {
                files.push(path);
            }
        }
    }
}

// ----------------------------------------------------
// 2. SemanticCadEngine (Electrónica)
// ----------------------------------------------------
pub struct SemanticCadEngine;

impl SemanticCadEngine {
    fn get_project_root(&self, backup_dir: &Path) -> PathBuf {
        backup_dir.parent()
            .and_then(|p| p.parent())
            .and_then(|p| p.parent())
            .and_then(|p| p.parent())
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."))
    }
}

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

        let project_root = self.get_project_root(backup_dir);
        let objects_dir = project_root.join(".ito").join("objects");
        std::fs::create_dir_all(&objects_dir).ok();

        let filter = IgnoreFilter::new(path);
        let mut current_files = Vec::new();
        scan_directory_recursive(path, path, &filter, &mut current_files);

        let mut current_hashes = HashMap::new();
        for file_path in current_files {
            if let Ok(hash) = cas::store_file(&file_path, &objects_dir) {
                if let Ok(relative) = file_path.strip_prefix(path) {
                    current_hashes.insert(relative.to_string_lossy().to_string().replace('\\', "/"), hash);
                }
            }
        }

        std::fs::create_dir_all(backup_dir).ok();
        let manifest_content = serde_json::to_string_pretty(&current_hashes).unwrap_or_default();
        
        let backup_manifest = backup_dir.join("manifest.json");
        std::fs::write(&backup_manifest, &manifest_content).ok();

        std::fs::create_dir_all(cache_dir).ok();
        let cache_manifest = cache_dir.join("manifest.json");
        std::fs::write(&cache_manifest, &manifest_content).ok();

        if let Ok(entries) = std::fs::read_dir(cache_dir) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.is_file() && entry.file_name() != "manifest.json" {
                    std::fs::remove_file(p).ok();
                }
            }
        }

        for (rel_path, hash) in &current_hashes {
            let cache_dest = cache_dir.join(rel_path);
            cas::restore_file(hash, &cache_dest, &objects_dir).ok();
        }

        metadata.insert("manifest".to_string(), "manifest.json".to_string());

        Ok(CommitPayload {
            engine_name: self.name().to_string(),
            changes_detected,
            details,
            metadata,
        })
    }

    fn restore(&self, path: &Path, backup_dir: &Path, payload: &CommitPayload) -> Result<(), String> {
        let manifest_filename = payload.metadata.get("manifest").cloned().unwrap_or_else(|| "manifest.json".to_string());
        let manifest_path = backup_dir.join(manifest_filename);
        if !manifest_path.exists() {
            return Err(format!("No se encontró el manifiesto de respaldo: {}", manifest_path.display()));
        }

        let content = std::fs::read_to_string(&manifest_path)
            .map_err(|e| format!("Error al leer manifiesto: {}", e))?;
        let hashes: HashMap<String, String> = serde_json::from_str(&content)
            .map_err(|e| format!("Error al parsear manifiesto: {}", e))?;

        let project_root = self.get_project_root(backup_dir);
        let objects_dir = project_root.join(".ito").join("objects");

        let filter = IgnoreFilter::new(path);
        let mut existing_files = Vec::new();
        scan_directory_recursive(path, path, &filter, &mut existing_files);
        for file in existing_files {
            std::fs::remove_file(file).ok();
        }

        for (rel_path, hash) in hashes {
            let dest = path.join(rel_path);
            cas::restore_file(&hash, &dest, &objects_dir)?;
        }

        Ok(())
    }
}

// ----------------------------------------------------
// 3. FileHashEngine (Mecánica, Documentación, Manufactura)
// ----------------------------------------------------
pub struct FileHashEngine;

impl FileHashEngine {
    fn get_project_root(&self, backup_dir: &Path) -> PathBuf {
        backup_dir.parent()
            .and_then(|p| p.parent())
            .and_then(|p| p.parent())
            .and_then(|p| p.parent())
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."))
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
        let filter = IgnoreFilter::new(path);
        let mut current_files = Vec::new();
        scan_directory_recursive(path, path, &filter, &mut current_files);

        let mut current_hashes = HashMap::new();
        for file_path in current_files {
            if let Ok(hash) = cas::calculate_file_hash(&file_path) {
                if let Ok(relative) = file_path.strip_prefix(path) {
                    current_hashes.insert(relative.to_string_lossy().to_string().replace('\\', "/"), hash);
                }
            }
        }

        let manifest_path = cache_dir.join("manifest.json");
        let old_hashes: HashMap<String, String> = if manifest_path.exists() {
            let content = std::fs::read_to_string(&manifest_path).unwrap_or_default();
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

        let project_root = self.get_project_root(backup_dir);
        let objects_dir = project_root.join(".ito").join("objects");
        std::fs::create_dir_all(&objects_dir).ok();

        let filter = IgnoreFilter::new(path);
        let mut current_files = Vec::new();
        scan_directory_recursive(path, path, &filter, &mut current_files);

        let mut current_hashes = HashMap::new();
        for file_path in current_files {
            if let Ok(hash) = cas::store_file(&file_path, &objects_dir) {
                if let Ok(relative) = file_path.strip_prefix(path) {
                    current_hashes.insert(relative.to_string_lossy().to_string().replace('\\', "/"), hash);
                }
            }
        }

        std::fs::create_dir_all(backup_dir).ok();
        let manifest_content = serde_json::to_string_pretty(&current_hashes).unwrap_or_default();
        
        let backup_manifest = backup_dir.join("manifest.json");
        std::fs::write(&backup_manifest, &manifest_content).ok();

        std::fs::create_dir_all(cache_dir).ok();
        let cache_manifest = cache_dir.join("manifest.json");
        std::fs::write(&cache_manifest, &manifest_content).ok();

        if let Ok(entries) = std::fs::read_dir(cache_dir) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.is_file() && entry.file_name() != "manifest.json" {
                    std::fs::remove_file(p).ok();
                }
            }
        }

        for (rel_path, hash) in &current_hashes {
            let cache_dest = cache_dir.join(rel_path);
            cas::restore_file(hash, &cache_dest, &objects_dir).ok();
        }

        metadata.insert("manifest".to_string(), "manifest.json".to_string());

        Ok(CommitPayload {
            engine_name: self.name().to_string(),
            changes_detected,
            details,
            metadata,
        })
    }

    fn restore(&self, path: &Path, backup_dir: &Path, payload: &CommitPayload) -> Result<(), String> {
        let manifest_filename = payload.metadata.get("manifest").cloned().unwrap_or_else(|| "manifest.json".to_string());
        let manifest_path = backup_dir.join(manifest_filename);
        if !manifest_path.exists() {
            return Err(format!("No se encontró el manifiesto de respaldo: {}", manifest_path.display()));
        }

        let content = std::fs::read_to_string(&manifest_path)
            .map_err(|e| format!("Error al leer manifiesto: {}", e))?;
        let hashes: HashMap<String, String> = serde_json::from_str(&content)
            .map_err(|e| format!("Error al parsear manifiesto: {}", e))?;

        let project_root = self.get_project_root(backup_dir);
        let objects_dir = project_root.join(".ito").join("objects");

        let filter = IgnoreFilter::new(path);
        let mut existing_files = Vec::new();
        scan_directory_recursive(path, path, &filter, &mut existing_files);
        for file in existing_files {
            std::fs::remove_file(file).ok();
        }

        for (rel_path, hash) in hashes {
            let dest = path.join(rel_path);
            cas::restore_file(&hash, &dest, &objects_dir)?;
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