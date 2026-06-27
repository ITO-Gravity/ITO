mod models;
mod parsers;
mod diff;

use clap::{Parser, Subcommand};
use anyhow::Result;

use ito::Config;

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
    /// Abre la interfaz gráfica de escritorio de Ito
    Gui,
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
            match ito::run_push(current_dir, message.clone()).await {
                Ok((_, info_msg)) => {
                    use colored::Colorize;
                    println!("{}", info_msg.green().bold());
                }
                Err(err_msg) => {
                    if err_msg.contains("No hay cambios pendientes") {
                        use colored::Colorize;
                        println!("{}", err_msg.green().bold());
                    } else {
                        anyhow::bail!("{}", err_msg);
                    }
                }
            }
        }
        Commands::Gui => {
            let current_exe = std::env::current_exe()?;
            let exe_dir = current_exe.parent().unwrap();
            let gui_exe = exe_dir.join("ito-gui.exe");

            if gui_exe.exists() {
                println!("Iniciando la Interfaz Gráfica de Ito...");
                std::process::Command::new(gui_exe).spawn()?;
            } else {
                println!("No se encontró el ejecutable de la GUI. Intentando iniciar en modo desarrollo...");
                let current_dir = std::env::current_dir()?;
                let gui_project_dir = current_dir.join("ito-gui");
                if gui_project_dir.exists() {
                    let mut cmd = std::process::Command::new("cmd.exe");
                    cmd.arg("/c")
                        .arg("npm run tauri dev")
                        .current_dir(gui_project_dir);
                    cmd.spawn()?;
                } else {
                    anyhow::bail!("Error: No se encontró el binario 'ito-gui.exe' ni la carpeta del proyecto GUI.");
                }
            }
        }
    }

    Ok(())
}
