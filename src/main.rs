use ito::{models, parsers, diff, linter, Config};
use clap::{Parser, Subcommand};
use anyhow::Result;

#[derive(Parser)]
#[command(name = "ito")]
#[command(about = "Ito: Motor de versionado semántico para ingeniería de hardware", long_about = None)]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum WorkspaceSubcommand {
    /// Cambia la ubicación del Workspace de ITO
    Set {
        /// Nueva ruta absoluta para el Workspace
        path: Option<String>,
    },
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

        /// Forzar el push omitiendo errores críticos del linter
        #[arg(long)]
        force: bool,
    },
    /// Ejecuta reglas de diseño eléctrico semántico (ERC)
    Lint {
        /// Opcional: Impedir impresiones detalladas, retornando únicamente el estado del sistema
        #[arg(short, long)]
        quiet: bool,
    },
    /// Crea una estructura de carpetas estándar para un nuevo proyecto multidisciplinar
    New {
        /// Nombre del nuevo proyecto
        name: String,
    },
    /// Administra el Workspace global de ITO
    Workspace {
        #[command(subcommand)]
        subcommand: Option<WorkspaceSubcommand>,
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
            println!("Analizando estado semántico del hardware...");
            
            let design = parsers::parse_project_directory(&current_dir)?;
            let cad_comp_count = design.components.len();
            let net_count = design.nets.len();

            println!("  CAD/Esquema: {} componentes cargados.", cad_comp_count);
            println!("  Nets: {} conexiones eléctricas encontradas.", net_count);
        }
        Commands::Diff { path, json } => {
            let current_dir = std::env::current_dir()?;

            // 1. Cargar diseño viejo (OLD) desde la caché oculta
            let cache_dir = current_dir.join(".ito").join("cache");
            let old_design = if cache_dir.exists() {
                parsers::parse_project_directory(&cache_dir).unwrap_or_else(|_| models::HardwareDesign::new())
            } else {
                models::HardwareDesign::new()
            };

            // 2. Cargar diseño nuevo (NEW)
            let new_design = parsers::parse_project_directory(&current_dir)?;

            // 3. Ejecutar comparación
            let diff_result = diff::diff_designs(&old_design, &new_design);

            if *json {
                let project_id = current_dir
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("ito-project")
                    .to_string();

                let design_json_content = serde_json::to_string_pretty(&new_design)?;
                
                let mut wtr = csv::Writer::from_writer(Vec::new());
                wtr.write_record(&["Designator", "MPN", "Manufacturer", "Value", "Footprint"]).ok();
                for (des, comp) in &new_design.components {
                    wtr.write_record(&[
                        des.as_str(),
                        comp.mpn.as_deref().unwrap_or(""),
                        comp.manufacturer.as_deref().unwrap_or(""),
                        comp.value.as_deref().unwrap_or(""),
                        comp.footprint.as_deref().unwrap_or(""),
                    ]).ok();
                }
                let bom_csv_content = Some(String::from_utf8(wtr.into_inner().unwrap_or_default())?);

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
        Commands::Push { message, force } => {
            let current_dir = std::env::current_dir()?;
            
            // Ejecutar linter antes de subir
            if let Ok(design) = parsers::parse_project_directory(&current_dir) {
                let issues = linter::run_lint(&design);
                let critical_count = issues.iter().filter(|i| i.severity == linter::LintSeverity::Critical).count();
                if critical_count > 0 && !*force {
                    use colored::Colorize;
                    println!("{}", "❌ Error: Se detectaron errores críticos en el diseño de hardware:".red().bold());
                    for issue in &issues {
                        if issue.severity == linter::LintSeverity::Critical {
                            println!("  - [{}] {}", issue.rule_id.red().bold(), issue.message);
                            println!("    {}", issue.details.dimmed());
                        }
                    }
                    println!("\n{}", "Push cancelado. Corrige los errores o usa '--force' para ignorarlos y continuar.".yellow().bold());
                    anyhow::bail!("Push abortado debido a errores ERC del linter.");
                }
            }

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
        Commands::Lint { quiet } => {
            let current_dir = std::env::current_dir()?;
            let design = parsers::parse_project_directory(&current_dir)?;
            let issues = linter::run_lint(&design);
            
            let critical_count = issues.iter().filter(|i| i.severity == linter::LintSeverity::Critical).count();
            let warning_count = issues.iter().filter(|i| i.severity == linter::LintSeverity::Warning).count();

            if !*quiet {
                use colored::Colorize;
                println!("{}", "=== REGLAS ELÉCTRICAS DE DISEÑO (ERC) ===".bold());
                if issues.is_empty() {
                    println!("{}", "✅ No se detectó ninguna anomalía en el diseño.".green().bold());
                } else {
                    for issue in &issues {
                        match issue.severity {
                            linter::LintSeverity::Critical => {
                                println!("\n🔴 [CRITICAL] [{}] {}", issue.rule_id.red().bold(), issue.message.red());
                                println!("   {}", issue.details.dimmed());
                            }
                            linter::LintSeverity::Warning => {
                                println!("\n🟡 [WARNING] [{}] {}", issue.rule_id.yellow().bold(), issue.message.yellow());
                                println!("   {}", issue.details.dimmed());
                            }
                            linter::LintSeverity::Info => {
                                println!("\n🔵 [INFO] [{}] {}", issue.rule_id.blue().bold(), issue.message.blue());
                                println!("   {}", issue.details.dimmed());
                            }
                        }
                    }
                    println!("\n🔍 Resumen: {} crítico(s), {} advertencia(s).", 
                             critical_count.to_string().red().bold(), 
                             warning_count.to_string().yellow().bold());
                }
            }

            if critical_count > 0 {
                std::process::exit(1);
            }
        }
        Commands::New { name } => {
            let ws_config = match ito::load_workspace_config() {
                Ok(Some(cfg)) => cfg,
                Ok(None) => {
                    use std::io::{self, Write};
                    use colored::Colorize;
                    
                    println!("{}", "No existe un Workspace configurado.\n".yellow().bold());
                    println!("¿Dónde desea guardar sus proyectos?\n");
                    println!("[1] Documentos/ITO (Recomendado)");
                    println!("[2] Elegir otra carpeta\n");
                    
                    let chosen_path = loop {
                        print!("Seleccione una opción: ");
                        io::stdout().flush().ok();
                        let mut option = String::new();
                        if io::stdin().read_line(&mut option).is_err() {
                            println!("{}", "Error al leer la entrada.".red());
                            std::process::exit(1);
                        }
                        let option = option.trim();
                        if option == "1" {
                            match ito::get_default_workspace_path() {
                                Ok(path) => {
                                    break path;
                                }
                                Err(err) => {
                                    println!("{}", format!("Error: {}", err).red());
                                    std::process::exit(1);
                                }
                            }
                        } else if option == "2" {
                            print!("Ingrese la ruta absoluta para el Workspace: ");
                            io::stdout().flush().ok();
                            let mut path_input = String::new();
                            if io::stdin().read_line(&mut path_input).is_err() {
                                println!("{}", "Error al leer la ruta.".red());
                                std::process::exit(1);
                            }
                            break std::path::PathBuf::from(path_input.trim());
                        } else {
                            println!("{}", "Opción inválida. Intente de nuevo.".yellow());
                        }
                    };
                    if let Err(err) = ito::save_workspace_config(&chosen_path) {
                        println!("{}", format!("❌ Error al guardar la configuración del Workspace: {}", err).red().bold());
                        std::process::exit(1);
                    }
                    
                    println!("✔ Workspace configurado en: {}\n", chosen_path.display().to_string().cyan());
                    
                    ito::models::ItoWorkspaceConfig {
                        workspace: chosen_path.to_string_lossy().to_string(),
                        version: "1.0".to_string(),
                    }
                }
                Err(err) => {
                    use colored::Colorize;
                    println!("{}", format!("❌ Error al cargar configuración global: {}", err).red().bold());
                    std::process::exit(1);
                }
            };

            let ws_path = std::path::PathBuf::from(&ws_config.workspace);
            let projects_dir = ws_path.join("Projects");

            match ito::run_new(projects_dir, name) {
                Ok((path, uuid)) => {
                    use colored::Colorize;
                    println!("✔ Proyecto creado correctamente.\n");
                    println!("Proyecto: {}", name.cyan().bold());
                    println!("UUID: {}", uuid.cyan());
                    println!("Ubicación: {}\n", path.display().to_string().cyan());
                    println!("{}", "ITO está listo para comenzar el versionado.".green().bold());
                }
                Err(err) => {
                    use colored::Colorize;
                    println!("{}", format!("❌ Error: {}", err).red().bold());
                    std::process::exit(1);
                }
            }
        }
        Commands::Workspace { subcommand } => {
            match subcommand {
                None => {
                    use colored::Colorize;
                    match ito::load_workspace_config() {
                        Ok(Some(cfg)) => {
                            let ws_path = std::path::PathBuf::from(&cfg.workspace);
                            let count = ito::run_workspace_get_count(&ws_path);
                            println!("{}", "Workspace actual".bold());
                            println!("{}\n", ws_path.display().to_string().cyan());
                            println!("{}", "Cantidad de proyectos:".bold());
                            println!("{}", count.to_string().cyan().bold());
                        }
                        Ok(None) => {
                            println!("{}", "No hay ningún Workspace configurado actualmente.".yellow());
                            println!("Ejecuta 'ito new <NombreProyecto>' o 'ito workspace set' para configurarlo.");
                        }
                        Err(err) => {
                            println!("{}", format!("❌ Error: {}", err).red().bold());
                            std::process::exit(1);
                        }
                    }
                }
                Some(WorkspaceSubcommand::Set { path }) => {
                    use std::io::{self, Write};
                    use colored::Colorize;
                    
                    let chosen_path = match path {
                        Some(p) => std::path::PathBuf::from(p),
                        None => {
                            print!("Ingrese la nueva ruta absoluta para el Workspace: ");
                            io::stdout().flush().ok();
                            let mut path_input = String::new();
                            if io::stdin().read_line(&mut path_input).is_err() {
                                println!("{}", "Error al leer la ruta.".red());
                                std::process::exit(1);
                            }
                            std::path::PathBuf::from(path_input.trim())
                        }
                    };

                    if let Err(err) = ito::save_workspace_config(&chosen_path) {
                        println!("{}", format!("❌ Error al guardar el nuevo Workspace: {}", err).red().bold());
                        std::process::exit(1);
                    }

                    println!("✔ Workspace actualizado correctamente a: {}\n", chosen_path.display().to_string().cyan());
                    println!("{}", "Nota: Los proyectos existentes no han sido movidos automáticamente.".yellow());
                }
            }
        }
    }

    Ok(())
}
