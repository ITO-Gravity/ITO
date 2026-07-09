use std::path::PathBuf;
use anyhow::{Result, Context};
use serde::{Deserialize, Serialize};
use chrono::{DateTime, Utc};

const REPO_OWNER: &str = "ITO-Gravity";
const REPO_NAME: &str = "ITO";
const USER_AGENT: &str = "ito-cli-updater";

#[derive(Serialize, Deserialize, Debug)]
struct UpdateCache {
    last_check: DateTime<Utc>,
    latest_version: String,
}

/// Compara si la versión `latest` es superior numéricamente a `current`
fn is_version_newer(current: &str, latest: &str) -> bool {
    let clean_curr = current.trim_start_matches('v');
    let clean_late = latest.trim_start_matches('v');

    // Separar sufijo de pre-release (ej. "0.1.0-alpha" -> "0.1.0", Some("alpha"))
    let mut curr_parts_split = clean_curr.splitn(2, '-');
    let curr_main = curr_parts_split.next().unwrap_or("");
    let curr_pre = curr_parts_split.next();

    let mut late_parts_split = clean_late.splitn(2, '-');
    let late_main = late_parts_split.next().unwrap_or("");
    let late_pre = late_parts_split.next();

    let curr_parts: Vec<u32> = curr_main.split('.').map(|s| s.parse().unwrap_or(0)).collect();
    let late_parts: Vec<u32> = late_main.split('.').map(|s| s.parse().unwrap_or(0)).collect();

    for i in 0..std::cmp::max(curr_parts.len(), late_parts.len()) {
        let curr_val = curr_parts.get(i).cloned().unwrap_or(0);
        let late_val = late_parts.get(i).cloned().unwrap_or(0);
        if late_val > curr_val {
            return true;
        } else if curr_val > late_val {
            return false;
        }
    }

    // Si las versiones principales son idénticas, comparamos pre-releases.
    // Según SemVer: "1.0.0-alpha" < "1.0.0" (la versión sin pre-release es más nueva)
    match (curr_pre, late_pre) {
        (Some(_), None) => true,      // latest no tiene pre-release (es más nuevo)
        (None, Some(_)) => false,     // current no tiene pre-release (es más nuevo)
        (Some(c), Some(l)) => l > c,  // Ambos tienen pre-release, se compara alfabéticamente
        (None, None) => false,        // Totalmente idénticos
    }
}


/// Obtiene la ruta del archivo de caché ~/.ito/update_check.json
fn get_cache_path() -> Result<PathBuf> {
    let home = if cfg!(target_os = "windows") {
        std::env::var("USERPROFILE").ok()
    } else {
        std::env::var("HOME").ok()
    };
    let home_path = home.context("No se pudo determinar el directorio de inicio (Home) del usuario.")?;
    Ok(PathBuf::from(home_path).join(".ito").join("update_check.json"))
}

/// Helper para construir peticiones autenticadas si se detecta un token de GitHub en el entorno
fn build_request(client: &reqwest::Client, method: reqwest::Method, url: &str) -> reqwest::RequestBuilder {
    let mut req = client.request(method, url);
    if let Ok(token) = std::env::var("GITHUB_TOKEN").or_else(|_| std::env::var("GH_TOKEN")) {
        let token = token.trim();
        if !token.is_empty() {
            req = req.bearer_auth(token);
        }
    }
    req
}

/// Comprueba si hay una nueva versión disponible en GitHub
pub async fn check_for_updates(force: bool) -> Result<Option<String>> {
    let current_version = env!("CARGO_PKG_VERSION");
    let cache_path = get_cache_path()?;

    if !force && cache_path.exists() {
        if let Ok(content) = std::fs::read_to_string(&cache_path) {
            if let Ok(cache) = serde_json::from_str::<UpdateCache>(&content) {
                let now = Utc::now();
                if now.signed_duration_since(cache.last_check).num_hours() < 24 {
                    // La caché es válida, comprobamos si ya sabemos de una nueva versión
                    if is_version_newer(current_version, &cache.latest_version) {
                        return Ok(Some(cache.latest_version));
                    }
                    return Ok(None);
                }
            }
        }
    }

    // Consulta rápida a la API de GitHub Releases con timeout de 3 segundos
    let url = format!("https://api.github.com/repos/{}/{}/releases/latest", REPO_OWNER, REPO_NAME);
    let client = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(std::time::Duration::from_secs(3))
        .build()?;

    let response = build_request(&client, reqwest::Method::GET, &url).send().await?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        anyhow::bail!("No se encontró ningún release en GitHub (404). Asegúrate de haber publicado al menos una versión en los Releases del repositorio.");
    }
    if !response.status().is_success() {
        anyhow::bail!("Fallo al consultar GitHub API: {}", response.status());
    }

    let release_info: serde_json::Value = response.json().await?;
    let latest_version = release_info.get("tag_name")
        .and_then(|v| v.as_str())
        .context("No se encontró el campo 'tag_name' en la respuesta del release")?
        .to_string();

    // Actualizar la caché local
    if let Some(parent) = cache_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let cache_data = UpdateCache {
        last_check: Utc::now(),
        latest_version: latest_version.clone(),
    };
    if let Ok(cache_str) = serde_json::to_string_pretty(&cache_data) {
        let _ = std::fs::write(&cache_path, cache_str);
    }

    if is_version_newer(current_version, &latest_version) {
        Ok(Some(latest_version))
    } else {
        Ok(None)
    }
}

/// Encuentra el URL de descarga del asset correspondiente al sistema y arquitectura actual
fn find_matching_asset(assets: &[serde_json::Value]) -> Option<String> {
    let target_os = std::env::consts::OS;
    let target_arch = std::env::consts::ARCH;

    let os_terms = match target_os {
        "windows" => vec!["windows", "win", "msvc", "pc-windows"],
        "macos" => vec!["macos", "darwin", "apple"],
        "linux" => vec!["linux", "unknown-linux"],
        _ => vec![target_os],
    };

    let arch_terms = match target_arch {
        "x86_64" => vec!["x86_64", "amd64", "x64"],
        "aarch64" => vec!["aarch64", "arm64"],
        _ => vec![target_arch],
    };

    for asset in assets {
        if let Some(name) = asset.get("name").and_then(|n| n.as_str()) {
            let name_lower = name.to_lowercase();
            let matches_os = os_terms.iter().any(|term| name_lower.contains(term));
            let matches_arch = arch_terms.iter().any(|term| name_lower.contains(term));
            
            let matches_ext = if target_os == "windows" {
                name_lower.ends_with(".exe") || name_lower.ends_with(".zip")
            } else {
                true
            };

            if matches_os && matches_arch && matches_ext {
                if let Some(url) = asset.get("browser_download_url").and_then(|u| u.as_str()) {
                    return Some(url.to_string());
                }
            }
        }
    }

    // Fallback: Si es Windows, buscar cualquier ejecutable .exe disponible
    for asset in assets {
        if let Some(name) = asset.get("name").and_then(|n| n.as_str()) {
            let name_lower = name.to_lowercase();
            if target_os == "windows" && name_lower.ends_with(".exe") {
                if let Some(url) = asset.get("browser_download_url").and_then(|u| u.as_str()) {
                    return Some(url.to_string());
                }
            }
        }
    }

    None
}

/// Extrae el binario desde un archivo ZIP comprimido
fn extract_binary_from_zip(zip_bytes: &[u8], target_bin_name: &str) -> Result<Vec<u8>> {
    use std::io::Read;
    let reader = std::io::Cursor::new(zip_bytes);
    let mut archive = zip::ZipArchive::new(reader)
        .context("Error al abrir el archivo ZIP descargado.")?;

    for i in 0..archive.len() {
        let mut file = archive.by_index(i)
            .context("Error al acceder al archivo dentro del ZIP.")?;
        
        let outpath = match file.enclosed_name() {
            Some(path) => path,
            None => continue,
        };

        let file_name = outpath.file_name().and_then(|n| n.to_str()).unwrap_or("");
        
        // Soporta que el binario se llame con o sin guión bajo (por ej. _ito.exe o ito.exe)
        let matches_name = file_name == target_bin_name || 
                           file_name == target_bin_name.trim_start_matches('_');

        if matches_name {
            let mut buffer = Vec::new();
            file.read_to_end(&mut buffer)
                .context("Error al extraer el binario desde el ZIP.")?;
            return Ok(buffer);
        }
    }
    anyhow::bail!("No se pudo encontrar el archivo binario '{}' dentro del archivo ZIP.", target_bin_name)
}

/// Realiza la descarga y reemplazo seguro del ejecutable
pub async fn download_and_install_update(target_version: &str) -> Result<()> {
    let url = format!("https://api.github.com/repos/{}/{}/releases/latest", REPO_OWNER, REPO_NAME);
    let client = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(std::time::Duration::from_secs(30))
        .build()?;

    let response = build_request(&client, reqwest::Method::GET, &url).send().await?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        anyhow::bail!("No se encontró ningún release en GitHub (404). Asegúrate de haber publicado al menos una versión en los Releases del repositorio.");
    }
    if !response.status().is_success() {
        anyhow::bail!("Fallo al consultar el release más reciente: {}", response.status());
    }

    let release_info: serde_json::Value = response.json().await?;
    let assets = release_info.get("assets")
        .and_then(|a| a.as_array())
        .context("No se encontraron assets en el release")?;

    let download_url = find_matching_asset(assets)
        .context("No se encontró un binario/zip compatible en el release de GitHub para tu sistema y arquitectura.")?;

    println!("Descargando actualización desde: {}", download_url);
    let asset_response = build_request(&client, reqwest::Method::GET, &download_url).send().await?;
    if !asset_response.status().is_success() {
        anyhow::bail!("Fallo al descargar el asset del release: {}", asset_response.status());
    }

    let bytes = asset_response.bytes().await?;

    let target_bin_name = if cfg!(target_os = "windows") {
        "_ito.exe"
    } else {
        "_ito"
    };

    let bin_bytes = if download_url.ends_with(".zip") {
        println!("Descomprimiendo binario...");
        extract_binary_from_zip(&bytes, target_bin_name)?
    } else {
        bytes.to_vec()
    };

    let current_exe = std::env::current_exe().context("No se pudo obtener la ruta del ejecutable en ejecución.")?;
    
    // Escribimos en un archivo temporal en el mismo directorio para asegurar renombrado atómico en el mismo disco
    let temp_exe = current_exe.with_extension("exe.tmp");
    std::fs::write(&temp_exe, &bin_bytes)
        .context("No se pudo escribir el archivo ejecutable temporal en el disco.")?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&temp_exe)?.permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&temp_exe, perms)?;
    }

    let old_exe = if cfg!(target_os = "windows") {
        current_exe.with_extension("exe.old")
    } else {
        current_exe.with_extension("old")
    };

    // Si ya existe un .old anterior, intentamos eliminarlo
    if old_exe.exists() {
        let _ = std::fs::remove_file(&old_exe);
    }

    // Renombrar el ejecutable activo actual (esto libera la ruta de ejecutable para escritura)
    std::fs::rename(&current_exe, &old_exe)
        .context("Fallo al renombrar el ejecutable activo actual.")?;

    // Mover el nuevo temporal a la ruta original
    if let Err(e) = std::fs::rename(&temp_exe, &current_exe) {
        // Rollback: restaurar el ejecutable anterior
        let _ = std::fs::rename(&old_exe, &current_exe);
        anyhow::bail!("Fallo al establecer el nuevo binario ejecutable: {}", e);
    }

    println!("¡ITO se ha actualizado correctamente a la versión v{}!", target_version.trim_start_matches('v'));
    Ok(())
}

/// Comprobación automática de fondo silenciosa (se ejecuta cada 24 horas)
pub async fn check_and_update_background() {
    if std::env::var("ITO_NO_AUTOUPDATE").is_ok() {
        return;
    }

    match check_for_updates(false).await {
        Ok(Some(new_version)) => {
            println!("¡Nueva versión de ITO detectada (v{})! Actualizando automáticamente...", new_version.trim_start_matches('v'));
            if let Err(e) = download_and_install_update(&new_version).await {
                eprintln!("Advertencia: Error al actualizar ITO automáticamente: {}", e);
            }
        }
        _ => {}
    }
}

/// Elimina el ejecutable .old remanente si existe
pub fn cleanup_old_executable() {
    if let Ok(current_exe) = std::env::current_exe() {
        let old_exe = if cfg!(target_os = "windows") {
            current_exe.with_extension("exe.old")
        } else {
            current_exe.with_extension("old")
        };
        if old_exe.exists() {
            let _ = std::fs::remove_file(old_exe);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_version_newer() {
        assert!(is_version_newer("0.1.0", "0.2.0"));
        assert!(is_version_newer("0.1.0", "v0.1.1"));
        assert!(is_version_newer("v0.1.0", "1.0.0"));
        assert!(is_version_newer("1.0.0", "1.0.1"));
        assert!(!is_version_newer("0.2.0", "0.1.0"));
        assert!(!is_version_newer("0.1.0", "0.1.0"));
        assert!(!is_version_newer("1.0.1", "1.0.1"));
        assert!(is_version_newer("0.1.0-alpha", "0.1.0"));
    }
}
