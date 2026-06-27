mod models;
mod parsers;
mod diff;

use clap::{Parser, Subcommand};
use anyhow::Result;

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct Config {
    project_id: String,
    remote_url: String,
}

#[derive(Debug, serde::Serialize, serde::Deserialize, Clone)]
struct CommitEntry {
    hash: String,
    parent_hash: String,
    message: String,
    timestamp: String,
    zip_path: String,
    synced: bool,
}

#[derive(Debug, Default, serde::Serialize, serde::Deserialize, Clone)]
struct History {
    commits: Vec<CommitEntry>,
}

#[derive(Parser)]
#[command(name = "ito")]
#[command(about = "Ito: Motor de versionado semántico para ingeniería de hardware", long_about = None)]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Inicializa un repositorio de Ito en el directorio actual
    Init,
    /// Muestra el estado del área de trabajo (BOM, CAD, Firmware)
    Status,
    /// Muestra los cambios semánticos detallados entre versiones
    Diff {
        /// Opcional: Especificar archivo o componente para comparar
        #[arg(short, long)]
        path: Option<String>,

        /// Generar reporte en formato JSON catalogado
        #[arg(long)]
        json: bool,
    },
    /// Sincroniza y respalda los archivos locales con la nube de Alexandria-HQ
    Push {
        /// Mensaje descriptivo para el commit/respaldo
        #[arg(short, long)]
        message: Option<String>,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match &cli.command {
        Commands::Init => {
            let current_dir = std::env::current_dir()?;
            let ito_dir = current_dir.join(".ito");
            
            if !ito_dir.exists() {
                std::fs::create_dir_all(&ito_dir)?;
            }

            let config_path = ito_dir.join("config.toml");
            if !config_path.exists() {
                let project_name = current_dir
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("ito-project")
                    .to_string();

                let default_config = Config {
                    project_id: project_name,
                    remote_url: "https://api.alexandria-hq.com/v1/reports".to_string(),
                };

                let toml_str = toml::to_string_pretty(&default_config)?;
                std::fs::write(&config_path, toml_str)?;
                println!("Repositorio Ito inicializado con éxito. Configuración creada en '.ito/config.toml'.");
            } else {
                println!("El repositorio Ito ya estaba inicializado en este directorio.");
            }
        }
        Commands::Status => {
            let current_dir = std::env::current_dir()?;
            let cad_path = current_dir.join("design.json");
            let bom_path = current_dir.join("bom.csv");

            if !cad_path.exists() || !bom_path.exists() {
                anyhow::bail!(
                    "Error: Se requieren los archivos 'design.json' (CAD) y 'bom.csv' (BOM) en el directorio actual para analizar el hardware.\n\
                     Directorio de ejecución actual: {}\n\
                     Asegúrate de que ambos archivos existan en la carpeta de ejecución de Ito.",
                     current_dir.display()
                );
            }

            println!("Analizando estado semántico del hardware...");
            
            // 1. Cargar diseño CAD (estructura física/eléctrica)
            let mut design = parsers::parse_cad_json(cad_path)?;
            let cad_comp_count = design.components.len();
            let net_count = design.nets.len();

            // 2. Cargar Lista de Materiales (BOM)
            let bom = parsers::parse_bom_csv(bom_path)?;

            // 3. Fusión semántica
            let (merged_count, missing_in_cad) = design.merge_bom(bom);

            println!("  BOM: {} componentes enriquecidos.", merged_count);
            println!("  CAD: {} componentes cargados.", cad_comp_count);
            println!("  Nets: {} conexiones eléctricas encontradas.", net_count);
            
            if !missing_in_cad.is_empty() {
                println!("  ⚠️  Advertencia: Se encontraron {} componentes en la BOM que no existen en el archivo CAD:", missing_in_cad.len());
                for des in missing_in_cad {
                    println!("    - {}", des);
                }
            }
        }
        Commands::Diff { path, json } => {
            let current_dir = std::env::current_dir()?;
            let new_cad = current_dir.join("design.json");
            let new_bom = current_dir.join("bom.csv");

            if !new_cad.exists() {
                anyhow::bail!(
                    "Error: Se requiere el archivo 'design.json' en el directorio actual para calcular las diferencias."
                );
            }

            // 1. Cargar diseño viejo (OLD) desde la caché oculta
            let cache_dir = current_dir.join(".ito").join("cache");
            let old_cad = cache_dir.join("design.old.json");
            let old_bom = cache_dir.join("bom.old.csv");

            let old_design = if old_cad.exists() {
                let mut design = parsers::parse_cad_json(&old_cad)?;
                if old_bom.exists() {
                    let bom = parsers::parse_bom_csv(&old_bom)?;
                    design.merge_bom(bom);
                }
                design
            } else {
                models::HardwareDesign::new()
            };

            // 2. Cargar diseño nuevo (NEW)
            let mut new_design = parsers::parse_cad_json(&new_cad)?;
            if new_bom.exists() {
                let bom = parsers::parse_bom_csv(&new_bom)?;
                new_design.merge_bom(bom);
            }

            // 3. Ejecutar comparación
            let diff_result = diff::diff_designs(&old_design, &new_design);

            if *json {
                let project_id = current_dir
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("ito-project")
                    .to_string();

                let design_json_content = std::fs::read_to_string(&new_cad)?;
                let bom_csv_content = if new_bom.exists() {
                    Some(std::fs::read_to_string(&new_bom)?)
                } else {
                    None
                };

                let report = diff::ItoReport::new(
                    project_id,
                    diff_result,
                    new_design,
                    design_json_content,
                    bom_csv_content,
                );
                let json_output = serde_json::to_string_pretty(&report)?;
                println!("{}", json_output);
                return Ok(());
            }

            use colored::Colorize;

            if diff_result.is_empty() {
                println!("{}", "Los diseños son semánticamente idénticos.".green().bold());
                return Ok(());
            }

            let filter = path.as_deref();
            if let Some(f) = filter {
                println!("{} {}", "Filtrando diferencias para:".bold(), f.cyan().bold());
            } else {
                println!("{}", "=== COMPARACIÓN SEMÁNTICA DE HARDWARE ===".bold());
            }

            // 1. Componentes Añadidos
            let mut printed_added_comp = false;
            for (des, comp) in &diff_result.components.added {
                if filter.is_none() || filter == Some(des) {
                    if !printed_added_comp {
                        println!("\n{}", "[Componentes Añadidos]".green().bold());
                        printed_added_comp = true;
                    }
                    println!(
                        "  {} {} (Value: {}, Footprint: {})",
                        "+".green().bold(),
                        des.green().bold(),
                        comp.value.as_deref().unwrap_or("N/A"),
                        comp.footprint.as_deref().unwrap_or("N/A")
                    );
                }
            }

            // 2. Componentes Eliminados
            let mut printed_deleted_comp = false;
            for (des, comp) in &diff_result.components.deleted {
                if filter.is_none() || filter == Some(des) {
                    if !printed_deleted_comp {
                        println!("\n{}", "[Componentes Eliminados]".red().bold());
                        printed_deleted_comp = true;
                    }
                    println!(
                        "  {} {} (Value: {}, Footprint: {})",
                        "-".red().bold(),
                        des.red().bold(),
                        comp.value.as_deref().unwrap_or("N/A"),
                        comp.footprint.as_deref().unwrap_or("N/A")
                    );
                }
            }

            // 3. Componentes Modificados
            let mut printed_mod_comp = false;
            for (des, comp_diff) in &diff_result.components.modified {
                if filter.is_none() || filter == Some(des) {
                    if !printed_mod_comp {
                        println!("\n{}", "[Componentes Modificados]".yellow().bold());
                        printed_mod_comp = true;
                    }
                    println!("  {} {}", "~".yellow().bold(), des.yellow().bold());
                    for change in &comp_diff.changes {
                        match change {
                            diff::ComponentChange::Mpn { old, new } => {
                                println!(
                                    "    * MPN cambiado de {} a {}",
                                    format!("{:?}", old).red(),
                                    format!("{:?}", new).green()
                                );
                            }
                            diff::ComponentChange::Manufacturer { old, new } => {
                                println!(
                                    "    * Fabricante cambiado de {} a {}",
                                    format!("{:?}", old).red(),
                                    format!("{:?}", new).green()
                                );
                            }
                            diff::ComponentChange::Value { old, new } => {
                                println!(
                                    "    * Valor cambiado de {} a {}",
                                    format!("{:?}", old).red(),
                                    format!("{:?}", new).green()
                                );
                            }
                            diff::ComponentChange::Footprint { old, new } => {
                                println!(
                                    "    * Footprint cambiado de {} a {}",
                                    format!("{:?}", old).red(),
                                    format!("{:?}", new).green()
                                );
                            }
                            diff::ComponentChange::PinAdded { pin_id, pin_name } => {
                                println!(
                                    "    * Pin {} ({}) {}",
                                    pin_id.green(),
                                    pin_name,
                                    "añadido".green()
                                );
                            }
                            diff::ComponentChange::PinDeleted { pin_id, pin_name } => {
                                println!(
                                    "    * Pin {} ({}) {}",
                                    pin_id.red(),
                                    pin_name,
                                    "eliminado".red()
                                );
                            }
                            diff::ComponentChange::PinModified { pin_id, changes } => {
                                println!("    * Pin {} modificado:", pin_id.yellow());
                                for pc in changes {
                                    match pc {
                                        diff::PinChange::Name { old, new } => {
                                            println!("      - Nombre cambiado de {} a {}", old.red(), new.green());
                                        }
                                        diff::PinChange::ElectricalType { old, new } => {
                                            println!("      - Tipo eléctrico cambiado de {:?} a {:?}", old, new);
                                        }
                                    }
                                }
                            }
                            diff::ComponentChange::AttributeAdded { key, val } => {
                                println!("    * Atributo {} con valor {} {}", key.green(), val, "añadido".green());
                            }
                            diff::ComponentChange::AttributeDeleted { key, val } => {
                                println!("    * Atributo {} con valor {} {}", key.red(), val, "eliminado".red());
                            }
                            diff::ComponentChange::AttributeModified { key, old, new } => {
                                println!("    * Atributo {} cambiado de {} a {}", key.yellow(), old.red(), new.green());
                            }
                        }
                    }
                }
            }

            // 4. Nets Añadidas
            let mut printed_added_nets = false;
            for (name, net) in &diff_result.nets.added {
                if filter.is_none() || filter == Some(name) {
                    if !printed_added_nets {
                        println!("\n{}", "[Nets Añadidas]".green().bold());
                        printed_added_nets = true;
                    }
                    println!(
                        "  {} {} (Endpoints: {})",
                        "+".green().bold(),
                        name.green().bold(),
                        net.endpoints.len()
                    );
                }
            }

            // 5. Nets Eliminadas
            let mut printed_deleted_nets = false;
            for (name, net) in &diff_result.nets.deleted {
                if filter.is_none() || filter == Some(name) {
                    if !printed_deleted_nets {
                        println!("\n{}", "[Nets Eliminadas]".red().bold());
                        printed_deleted_nets = true;
                    }
                    println!(
                        "  {} {} (Endpoints: {})",
                        "-".red().bold(),
                        name.red().bold(),
                        net.endpoints.len()
                    );
                }
            }

            // 6. Nets Modificadas (Mutaciones Eléctricas)
            let mut printed_mod_nets = false;
            for (name, net_diff) in &diff_result.nets.modified {
                // Se muestra la net si coincide con el filtro o si el filtro coincide con alguno de sus endpoints
                let matches_endpoint = filter.map_or(false, |f| {
                    net_diff.added_endpoints.iter().any(|ep| ep.component_designator == f)
                        || net_diff.deleted_endpoints.iter().any(|ep| ep.component_designator == f)
                });
                
                if filter.is_none() || filter == Some(name) || matches_endpoint {
                    if !printed_mod_nets {
                        println!("\n{}", "[Nets Modificadas (Mutaciones Eléctricas)]".yellow().bold());
                        printed_mod_nets = true;
                    }
                    println!("  {} {}", "~".yellow().bold(), name.yellow().bold());
                    for ep in &net_diff.added_endpoints {
                        println!(
                            "    {} Endpoint conectado: {}:{}",
                            "+".green().bold(),
                            ep.component_designator.green(),
                            ep.pin_id.green()
                        );
                    }
                    for ep in &net_diff.deleted_endpoints {
                        println!(
                            "    {} Endpoint desconectado: {}:{}",
                            "-".red().bold(),
                            ep.component_designator.red(),
                            ep.pin_id.red()
                        );
                    }
                }
            }

            println!("");
        }
        Commands::Push { message } => {
            let current_dir = std::env::current_dir()?;
            let config_path = current_dir.join(".ito").join("config.toml");

            if !config_path.exists() {
                anyhow::bail!(
                    "Error: No se encontró la configuración de Ito. ¿Corriste 'ito init' primero?"
                );
            }

            // 1. Leer configuración TOML
            let config_str = std::fs::read_to_string(&config_path)?;
            let config: Config = toml::from_str(&config_str)?;

            // 2. Comprobar archivos locales
            let new_cad = current_dir.join("design.json");
            let new_bom = current_dir.join("bom.csv");

            if !new_cad.exists() {
                anyhow::bail!(
                    "Error: Se requiere el archivo 'design.json' en el directorio actual para enviar."
                );
            }

            // 3. Calcular hash SHA-256 de los archivos
            use sha2::{Sha256, Digest};
            let design_bytes = std::fs::read(&new_cad)?;
            let mut hasher = Sha256::new();
            hasher.update(&design_bytes);
            
            if new_bom.exists() {
                let bom_bytes = std::fs::read(&new_bom)?;
                hasher.update(&bom_bytes);
            }
            let hash_result = hasher.finalize();
            let hash_str = format!("{:x}", hash_result);

            // 4. Cargar historial local
            let history_path = current_dir.join(".ito").join("history.toml");
            let mut history = if history_path.exists() {
                let content = std::fs::read_to_string(&history_path)?;
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
                use colored::Colorize;
                println!("{}", "No hay cambios pendientes de hardware para respaldar ni sincronizar.".green().bold());
                return Ok(());
            }

            println!("Generando respaldo local comprimido...");

            // 5. Crear la carpeta de respaldos y el ZIP
            let backups_dir = current_dir.join(".ito").join("backups");
            std::fs::create_dir_all(&backups_dir)?;
            let zip_path = backups_dir.join(format!("{}.zip", hash_str));
            let zip_file = std::fs::File::create(&zip_path)?;
            let mut zip = zip::ZipWriter::new(zip_file);

            let options = zip::write::FileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);

            // Adjuntar design.json al ZIP
            zip.start_file("design.json", options)?;
            use std::io::Write;
            zip.write_all(&design_bytes)?;

            // Adjuntar bom.csv al ZIP (si existe)
            if new_bom.exists() {
                let bom_bytes = std::fs::read(&new_bom)?;
                zip.start_file("bom.csv", options)?;
                zip.write_all(&bom_bytes)?;
            }
            zip.finish()?;

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
            println!("Sincronizando archivos del proyecto con Alexandria-HQ ({})...", config.remote_url);

            let mut form = reqwest::multipart::Form::new()
                .text("project_id", config.project_id.clone())
                .text("domain", "hardware")
                .text("version_hash", hash_str.clone())
                .text("parent_hash", parent_hash.clone())
                .text("message", commit_msg.clone());

            let design_part = reqwest::multipart::Part::bytes(design_bytes.clone())
                .file_name("design.json")
                .mime_str("application/json")?;
            form = form.part("design_json", design_part);

            if new_bom.exists() {
                let bom_bytes = std::fs::read(&new_bom)?;
                let bom_part = reqwest::multipart::Part::bytes(bom_bytes)
                    .file_name("bom.csv")
                    .mime_str("text/csv")?;
                form = form.part("bom_csv", bom_part);
            }

            let client = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(10))
                .build()?;
            let send_res = client
                .post(&config.remote_url)
                .multipart(form)
                .send()
                .await;

            use colored::Colorize;
            let mut sync_success = false;

            match send_res {
                Ok(response) => {
                    if response.status().is_success() {
                        sync_success = true;
                        println!(
                            "{}",
                            format!(
                                "¡Archivos del proyecto sincronizados con éxito en Alexandria-HQ! [Proyecto: {}]",
                                config.project_id
                            )
                            .green()
                            .bold()
                        );
                    } else {
                        println!(
                            "{}",
                            format!(
                                "Backup local generado con éxito en .ito/backups/. Sincronización fallida (HTTP {}). Sincronización pendiente.",
                                response.status()
                            )
                            .yellow()
                        );
                    }
                }
                Err(_) => {
                    println!(
                        "{}",
                        "Backup local generado con éxito en .ito/backups/. Sincronización con Alexandria-HQ pendiente (Servidor no disponible)"
                            .yellow()
                            .bold()
                    );
                }
            }

            commit_entry.synced = sync_success;
            history.commits.push(commit_entry);
            let history_str = toml::to_string_pretty(&history)?;
            std::fs::write(&history_path, history_str)?;

            // 8. Actualizar la caché local para 'ito diff'
            let cache_dir = current_dir.join(".ito").join("cache");
            std::fs::create_dir_all(&cache_dir)?;
            std::fs::copy(&new_cad, cache_dir.join("design.old.json"))?;
            if new_bom.exists() {
                std::fs::copy(&new_bom, cache_dir.join("bom.old.csv"))?;
            } else {
                let cached_old_bom = cache_dir.join("bom.old.csv");
                if cached_old_bom.exists() {
                    std::fs::remove_file(cached_old_bom)?;
                }
            }
        }
    }

    Ok(())
}
