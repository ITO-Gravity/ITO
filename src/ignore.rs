// src/ignore.rs

use std::path::Path;

pub struct IgnoreFilter {
    custom_patterns: Vec<String>,
}

impl IgnoreFilter {
    pub fn new(project_root: &Path) -> Self {
        let mut custom_patterns = Vec::new();
        let itoignore_path = project_root.join(".itoignore");
        if itoignore_path.exists() {
            if let Ok(content) = std::fs::read_to_string(&itoignore_path) {
                for line in content.lines() {
                    let trimmed = line.trim();
                    if !trimmed.is_empty() && !trimmed.starts_with('#') {
                        custom_patterns.push(trimmed.to_lowercase().replace('\\', "/"));
                    }
                }
            }
        }
        Self { custom_patterns }
    }

    pub fn is_ignored(&self, path: &Path) -> bool {
        let path_str = path.to_string_lossy().to_lowercase().replace('\\', "/");
        
        // 1. Verificación de patrones por defecto en segmentos de ruta
        let segments: Vec<&str> = path_str.split('/').collect();
        for seg in &segments {
            if *seg == ".git" 
                || *seg == "node_modules" 
                || *seg == ".pio" 
                || *seg == "target" 
                || *seg == ".venv" 
                || *seg == "bin" 
                || *seg == "obj" 
                || *seg == ".vs" 
                || *seg == "history"
                || *seg == "__pycache__"
                || *seg == ".ito"
                || *seg == "project backups"  // carpeta de respaldos automáticos de Proteus
            {
                return true;
            }

            if seg.ends_with("-backups") {
                return true;
            }
        }

        // 2. Verificación de nombre de archivo (temporales y bloqueos)
        if let Some(file_name) = path.file_name().and_then(|s| s.to_str()) {
            let file_name_lower = file_name.to_lowercase();
            if file_name_lower.ends_with(".lck") {
                return true;
            }
            if file_name_lower.starts_with("~$") || file_name_lower.starts_with(".~") {
                return true;
            }
            if file_name_lower.ends_with(".tmp") || file_name_lower.ends_with(".bak") {
                return true;
            }
            // Ruido de Proteus: respaldos (.pdsbak) y estado de sesión por máquina/usuario (.workspace)
            if file_name_lower.ends_with(".pdsbak") || file_name_lower.ends_with(".workspace") {
                return true;
            }
        }

        // 3. Verificación de patrones personalizados del .itoignore
        for pattern in &self.custom_patterns {
            if path_str.contains(pattern) {
                return true;
            }
        }

        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_default_ignores() {
        let filter = IgnoreFilter { custom_patterns: vec![] };

        assert!(filter.is_ignored(&PathBuf::from("firmware/.pio/build/main.o")));
        assert!(filter.is_ignored(&PathBuf::from("firmware/node_modules/lodash/index.js")));
        assert!(filter.is_ignored(&PathBuf::from("firmware/target/debug/ito")));
        assert!(filter.is_ignored(&PathBuf::from("firmware/bin/Debug/net6.0/app.dll")));
        assert!(filter.is_ignored(&PathBuf::from("electronics/.git/HEAD")));
        assert!(filter.is_ignored(&PathBuf::from("electronics/.ito/history.toml")));

        assert!(filter.is_ignored(&PathBuf::from("electronics/PCB.kicad_pcb.lck")));
        assert!(filter.is_ignored(&PathBuf::from("mechanical/~$housing.sldprt")));
        assert!(filter.is_ignored(&PathBuf::from("mechanical/.~housing.sldprt")));
        assert!(filter.is_ignored(&PathBuf::from("mechanical/History/housing_v1.zip")));
        assert!(filter.is_ignored(&PathBuf::from("electronics/project-backups/version1.zip")));

        assert!(filter.is_ignored(&PathBuf::from("electronics/project-backups-backups/kicad_pcb")));

        // Ruido de Proteus
        assert!(filter.is_ignored(&PathBuf::from("electronics/pcb/Project Backups/pruebas [20260718].pdsprj")));
        assert!(filter.is_ignored(&PathBuf::from("electronics/pcb/pruebas.pdsbak")));
        assert!(filter.is_ignored(&PathBuf::from("electronics/pcb/pruebas.pdsprj.DESKTOP-X.vaslo.workspace")));
        // El proyecto Proteus real NO se ignora
        assert!(!filter.is_ignored(&PathBuf::from("electronics/pcb/pruebas.pdsprj")));

        assert!(!filter.is_ignored(&PathBuf::from("firmware/src/main.cpp")));
        assert!(!filter.is_ignored(&PathBuf::from("electronics/main_board.kicad_pcb")));
    }
}