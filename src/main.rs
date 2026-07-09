use ito::{models, parsers, diff, linter, Config};
use clap::{Parser, Subcommand};
use anyhow::Result;
use colored::Colorize;

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
enum AuthSubcommand {
    /// Inicia sesión con un token de API de ITO
    Login {
        /// Token de API obtenido desde la web de ITO Gravity
        #[arg(long)]
        token: String,
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
    /// Guarda un nuevo respaldo de diseño local analizando los cambios semánticos
    Commit {
        /// Mensaje descriptivo para el commit/respaldo
        #[arg(short, long)]
        message: Option<String>,

        /// Forzar el commit omitiendo errores críticos del linter
        #[arg(long)]
        force: bool,

        /// Envía automáticamente el commit al servidor remoto tras crearlo localmente
        #[arg(long)]
        push: bool,
    },
    /// Muestra el historial completo de revisiones locales de hardware
    Log,
    /// Restaura una versión anterior del diseño de hardware en tu directorio de trabajo
    Restore {
        /// El hash (o prefijo del hash corto) de la versión a restaurar
        hash: String,
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
    /// Selecciona interactivamente un proyecto y copia la instrucción para navegar a él al portapapeles
    Select,
    /// Vincula un directorio físico externo de un CAD o IDE a un módulo del proyecto de ITO
    Link,
    /// Muestra la lista consolidada de enlaces y módulos vinculados en el proyecto
    Links,
    /// Copia al portapapeles la instrucción para navegar a un módulo vinculado (firmware, electronics, etc.)
    Go {
        /// Opcional: Nombre del módulo a navegar
        module: Option<String>,
    },
    /// Autentica el cliente ITO con el servidor
    Auth {
        #[command(subcommand)]
        subcommand: AuthSubcommand,
    },
    /// Envía la última versión local del proyecto al servidor remoto
    Push,
    /// Descarga la última versión registrada del proyecto desde el servidor remoto
    Pull,
    /// Clona un proyecto existente desde el servidor remoto usando su Token de API o tu sesión global
    Clone {
        /// Token de API del proyecto, o ID/nombre/URL del proyecto si ya iniciaste sesión
        token_or_project: String,
    },
    /// Inicia el asistente interactivo para guiar al operador paso a paso (alias de select)
    Guia,
    /// Inicia sesión con tus credenciales de ITOGravity (Email y Contraseña)
    Login,
    /// Inicia sesión con tus credenciales de ITOGravity (Email y Contraseña)
    Logueo,
    /// Comprueba y actualiza ITO a la última versión disponible desde GitHub
    Update {
        /// Fuerza la descarga y actualización omitiendo los límites de la caché
        #[arg(short, long)]
        force: bool,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let _ = ito::install_shell_wrappers();
    let cli = Cli::parse();

    // Comprobación y actualización automática en segundo plano (ejecución una vez al día)
    if !matches!(&cli.command, Commands::Update { .. }) {
        let _ = ito::updater::check_and_update_background().await;
    }

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
                    token: None,
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
            
            if let Some(root) = ito::find_project_root(&current_dir) {
                let ito_json_path = root.join("ito.json");
                let mut project_name = "Proyecto Ito".to_string();
                let mut links = std::collections::HashMap::new();

                if ito_json_path.exists() {
                    if let Ok(content) = std::fs::read_to_string(&ito_json_path) {
                        if let Ok(config) = serde_json::from_str::<models::ItoProjectConfig>(&content) {
                            project_name = config.project_name;
                            links = config.links.unwrap_or_default();
                        }
                    }
                }

                println!("\n{} {}", "Proyecto:".bold(), project_name.cyan().bold());
                println!("{} {}\n", "Raíz:".bold(), root.display().to_string().cyan());

                if links.is_empty() {
                    println!("{} (Sin módulos enlazados - Analizando raíz)", "Electrónica".bold());
                    match parsers::parse_project_directory(&current_dir) {
                        Ok(design) => {
                            println!("  [OK] CAD/Esquema: {} componentes cargados.", design.components.len());
                            println!("  [OK] Nets: {} conexiones eléctricas encontradas.", design.nets.len());
                        }
                        Err(e) => {
                            println!("  Warning: No se encontraron archivos de hardware válidos en la raíz: {}", e);
                        }
                    }
                } else {
                    let registry = ito::engines::EngineRegistry::new();
                    
                    let disciplines = [
                        ("firmware", "Firmware"),
                        ("electronics", "Electrónica"),
                        ("mechanical", "Mecánica"),
                        ("documentation", "Documentación"),
                        ("manufacturing", "Manufactura"),
                    ];

                    for &(key, name) in &disciplines {
                        if let Some(link) = links.get(key) {
                            let module_path = std::path::PathBuf::from(&link.path);
                            let engine = registry.get_engine(&link.engine)
                                .unwrap_or_else(|| registry.get_engine("file-hash").unwrap());
                            
                            let m_cache_dir = root.join(".ito").join("cache").join(key);
                            
                            let is_current = current_dir.to_string_lossy().to_lowercase().replace('\\', "/")
                                .starts_with(&module_path.to_string_lossy().to_lowercase().replace('\\', "/"));
                            
                            let current_marker = if is_current { " (consola aquí)" } else { "" };
                            println!("{}{}", name.bold(), current_marker.dimmed());

                            match engine.status(&module_path, &m_cache_dir) {
                                Ok(ito::engines::ModuleStatus::Unchanged) => {
                                    println!("  {}", "Sin cambios".green());
                                }
                                Ok(ito::engines::ModuleStatus::Modified { summary, details }) => {
                                    println!("  [MODIFIED] {}", summary.yellow());
                                    for detail in details.iter().take(5) {
                                        println!("    {}", detail.dimmed());
                                    }
                                    if details.len() > 5 {
                                        println!("    ... y {} cambios más.", details.len() - 5);
                                    }
                                }
                                Ok(ito::engines::ModuleStatus::Error(e)) => {
                                    println!("  Warning: Error de análisis: {}", e.red());
                                }
                                Err(e) => {
                                    println!("  Warning: {}", e.red());
                                }
                            }
                            println!();
                        }
                    }
                }
            } else {
                println!("{}", "Warning: No se detectó ninguna relación con un proyecto de Ito activo.".yellow());
            }

            println!("Note: Si realizaste modificaciones, puedes comparar los cambios semánticos con: {}", "ito diff".cyan());
            println!("Note: Si estás listo para guardar esta versión localmente, ejecuta: {}", "ito commit -m \"Mensaje\"".cyan());
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
            println!("Note: Si estás de acuerdo con estos cambios, puedes guardarlos localmente con: {}", "ito commit -m \"Mensaje\"".cyan());
        }
        Commands::Commit { message, force, push } => {
            let current_dir = std::env::current_dir()?;
            let project_root = ito::find_project_root(&current_dir).unwrap_or(current_dir.clone());
            let mut electronics_path = current_dir.clone();
            
            // Buscar si hay un link de electronics en ito.json
            let ito_json_path = project_root.join("ito.json");
            if ito_json_path.exists() {
                if let Ok(content) = std::fs::read_to_string(&ito_json_path) {
                    if let Ok(config) = serde_json::from_str::<models::ItoProjectConfig>(&content) {
                        if let Some(links) = config.links {
                            if let Some(link) = links.get("electronics") {
                                electronics_path = std::path::PathBuf::from(&link.path);
                            }
                        }
                    }
                }
            }

            // Ejecutar linter antes de hacer commit en la ruta de electrónica
            if let Ok(design) = parsers::parse_project_directory(&electronics_path) {
                let issues = linter::run_lint(&design);
                let critical_count = issues.iter().filter(|i| i.severity == linter::LintSeverity::Critical).count();
                if critical_count > 0 && !*force {
                    use colored::Colorize;
                    println!("{}", "Error: Se detectaron errores críticos en el diseño de hardware:".red().bold());
                    for issue in &issues {
                        if issue.severity == linter::LintSeverity::Critical {
                            println!("  - [{}] {}", issue.rule_id.red().bold(), issue.message);
                            println!("    {}", issue.details.dimmed());
                        }
                    }
                    println!("\n{}", "Commit cancelado. Corrige los errores o usa '--force' para ignorarlos y continuar.".yellow().bold());
                    anyhow::bail!("Commit abortado debido a errores ERC del linter.");
                }
            }

            match ito::run_commit(project_root.clone(), message.clone()) {
                Ok(commit) => {
                    use colored::Colorize;
                    println!("{}", "Respaldo de diseño guardado localmente con éxito.".green().bold());
                    println!("Hash:    {}", commit.hash.cyan());
                    println!("Mensaje: {}", commit.message.bold());
                    println!("Fecha:   {}", commit.timestamp.dimmed());

                    if !commit.modules.is_empty() {
                        println!("\nResumen por módulo:");
                        for (mod_name, payload) in &commit.modules {
                            let status_indicator = if payload.changes_detected { "[MODIFIED]".yellow() } else { "[OK]".green() };
                            println!("  {} [{}]: {}", status_indicator, mod_name.bold(), payload.engine_name.cyan());
                            for detail in &payload.details {
                                println!("    {}", detail.dimmed());
                            }
                        }
                    } else if let Some(ref summary) = commit.diff_summary {
                        println!("\nResumen de cambios:");
                        println!(
                            "  Componentes: {} añadidos, {} eliminados, {} modificados",
                            summary.added_components.to_string().green(),
                            summary.deleted_components.to_string().red(),
                            summary.modified_components.to_string().yellow()
                        );
                        println!(
                            "  Conexiones:  {} añadidas, {} eliminadas, {} modificadas",
                            summary.added_nets.to_string().green(),
                            summary.deleted_nets.to_string().red(),
                            summary.modified_nets.to_string().yellow()
                        );
                    }
                    println!("\nNote: Puedes ver el historial de versiones con: {}", "ito log".cyan());

                    if *push {
                        println!("\nIniciando push automático al servidor...");
                        match ito::run_push(project_root.clone()).await {
                            Ok(msg) => {
                                println!("{} Sincronización exitosa: {}", "OK".green().bold(), msg);
                            }
                            Err(e) => {
                                println!("{} Error al subir versión al servidor: {}", "ERROR".red().bold(), e);
                            }
                        }
                    }
                }
                Err(err_msg) => {
                    if err_msg.contains("No hay cambios pendientes") {
                        use colored::Colorize;
                        println!("{}", err_msg.yellow());
                    } else {
                        anyhow::bail!("{}", err_msg);
                    }
                }
            }
        }
        Commands::Log => {
            use colored::Colorize;

            let current_dir = std::env::current_dir()?;
            let root = match ito::find_project_root(&current_dir) {
                Some(r) => r,
                None => {
                    println!("{}", "Error: No se encontró la raíz del proyecto. ¿Ejecutaste 'ito init' o 'ito new' primero?".red().bold());
                    std::process::exit(1);
                }
            };

            let history_path = root.join(".ito").join("history.toml");
            if !history_path.exists() {
                println!("{}", "No hay ningún commit registrado en este repositorio todavía.".yellow());
                println!("\nNote: Comienza guardando una versión con: {}", "ito commit -m \"Mensaje\"".cyan());
                return Ok(());
            }

            let content = std::fs::read_to_string(&history_path)?;
            let history: ito::History = toml::from_str(&content).unwrap_or_default();

            if history.commits.is_empty() {
                println!("{}", "No hay ningún commit registrado en este repositorio todavía.".yellow());
                println!("\nNote: Comienza guardando una versión con: {}", "ito commit -m \"Mensaje\"".cyan());
                return Ok(());
            }

            println!("\n{}", "Historial de Revisiones de Hardware".bold());
            println!("------------------------------------------------------------");

            // Mostrar el último commit primero
            for commit in history.commits.iter().rev() {
                let short_hash = if commit.hash.len() > 8 {
                    &commit.hash[..8]
                } else {
                    &commit.hash
                };

                println!("Commit:  {}", short_hash.cyan().bold());
                println!("Fecha:   {}", commit.timestamp.dimmed());
                println!("Mensaje: {}", commit.message.bold());

                if let Some(ref summary) = commit.diff_summary {
                    let total_changes = summary.added_components
                        + summary.deleted_components
                        + summary.modified_components
                        + summary.added_nets
                        + summary.deleted_nets
                        + summary.modified_nets;

                    if total_changes > 0 {
                        print!("Cambios: ");
                        let mut parts = Vec::new();
                        if summary.added_components > 0 {
                            parts.push(format!("+{} comp", summary.added_components).green().to_string());
                        }
                        if summary.deleted_components > 0 {
                            parts.push(format!("-{} comp", summary.deleted_components).red().to_string());
                        }
                        if summary.modified_components > 0 {
                            parts.push(format!("~{} comp", summary.modified_components).yellow().to_string());
                        }
                        if summary.added_nets > 0 {
                            parts.push(format!("+{} nets", summary.added_nets).green().to_string());
                        }
                        if summary.deleted_nets > 0 {
                            parts.push(format!("-{} nets", summary.deleted_nets).red().to_string());
                        }
                        if summary.modified_nets > 0 {
                            parts.push(format!("~{} nets", summary.modified_nets).yellow().to_string());
                        }
                        println!("{}", parts.join(", "));
                    } else {
                        println!("Cambios: {}", "Sin cambios en componentes ni conexiones.".dimmed());
                    }
                }
                println!("------------------------------------------------------------");
            }

            println!("\nNote: Si deseas restaurar tu diseño a una versión anterior, ejecuta: {}", "ito restore <hash_corto>".cyan());
        }
        Commands::Restore { hash } => {
            use std::io::{self, Write};
            use colored::Colorize;

            let current_dir = std::env::current_dir()?;
            let root = match ito::find_project_root(&current_dir) {
                Some(r) => r,
                None => {
                    println!("{}", "Error: No se encontró la raíz del proyecto. ¿Ejecutaste 'ito init' o 'ito new' primero?".red().bold());
                    std::process::exit(1);
                }
            };

            // Verificar si hay cambios sin guardar comparando la caché con la carpeta de trabajo
            let cache_dir = root.join(".ito").join("cache");
            let old_design = if cache_dir.exists() {
                parsers::parse_project_directory(&cache_dir).unwrap_or_else(|_| models::HardwareDesign::new())
            } else {
                models::HardwareDesign::new()
            };
            let new_design = parsers::parse_project_directory(&root).unwrap_or_else(|_| models::HardwareDesign::new());
            let diff_result = diff::diff_designs(&old_design, &new_design);

            if !diff_result.is_empty() {
                println!("{}", "Warning: Tienes cambios no guardados en tu diseño de hardware actual.".yellow().bold());
                println!("Si restauras otra versión, perderás de forma permanente los cambios actuales.");
                print!("¿Deseas continuar de todas formas? [s/N]: ");
                io::stdout().flush().ok();

                let mut answer = String::new();
                if io::stdin().read_line(&mut answer).is_err() {
                    println!("{}", "Cancelado.".red());
                    std::process::exit(1);
                }
                let answer = answer.trim().to_lowercase();
                if answer != "s" && answer != "si" {
                    println!("{}", "Restauración cancelada.".yellow());
                    return Ok(());
                }
            }

            match ito::run_restore(root, hash) {
                Ok(restored_files) => {
                    println!("\n{}", "Diseño de hardware restaurado correctamente con éxito.".green().bold());
                    println!("Archivos recuperados:");
                    for file in restored_files {
                        println!("  - {}", file.cyan());
                    }
                    println!("\nNote: Puedes verificar el estado de tu diseño con: {}", "ito status".cyan());
                }
                Err(err) => {
                    println!("{}", format!("Error: {}", err).red().bold());
                    std::process::exit(1);
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
                    println!("{}", "No se detectó ninguna anomalía en el diseño.".green().bold());
                } else {
                    for issue in &issues {
                        match issue.severity {
                            linter::LintSeverity::Critical => {
                                println!("\n[CRITICAL] [{}] {}", issue.rule_id.red().bold(), issue.message.red());
                                println!("   {}", issue.details.dimmed());
                            }
                            linter::LintSeverity::Warning => {
                                println!("\n[WARNING] [{}] {}", issue.rule_id.yellow().bold(), issue.message.yellow());
                                println!("   {}", issue.details.dimmed());
                            }
                            linter::LintSeverity::Info => {
                                println!("\n[INFO] [{}] {}", issue.rule_id.blue().bold(), issue.message.blue());
                                println!("   {}", issue.details.dimmed());
                            }
                        }
                    }
                    println!("\nResumen: {} crítico(s), {} advertencia(s).", 
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
                        println!("{}", format!("Error al guardar la configuración del Workspace: {}", err).red().bold());
                        std::process::exit(1);
                    }
                    
                    println!("Workspace configurado en: {}\n", chosen_path.display().to_string().cyan());
                    
                    ito::models::ItoWorkspaceConfig {
                        workspace: chosen_path.to_string_lossy().to_string(),
                        version: "1.0".to_string(),
                        token: None,
                    }
                }
                Err(err) => {
                    use colored::Colorize;
                    println!("{}", format!("Error al cargar configuración global: {}", err).red().bold());
                    std::process::exit(1);
                }
            };

            let ws_path = std::path::PathBuf::from(&ws_config.workspace);
            let projects_dir = ws_path.join("Projects");

            match ito::run_new(projects_dir, name) {
                Ok((path, uuid)) => {
                    use colored::Colorize;
                    println!("Proyecto creado correctamente.\n");
                    println!("Proyecto: {}", name.cyan().bold());
                    println!("UUID: {}", uuid.cyan());
                    println!("Ubicación: {}\n", path.display().to_string().cyan());
                    println!("{}", "ITO está listo para comenzar el versionado.".green().bold());
                    println!("\nNote: Ingresa a la carpeta del proyecto y vincula tus módulos con: {}", "ito link".cyan());
                }
                Err(err) => {
                    use colored::Colorize;
                    println!("{}", format!("Error: {}", err).red().bold());
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
                            println!("{}", format!("Error: {}", err).red().bold());
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
                        println!("{}", format!("Error al guardar el nuevo Workspace: {}", err).red().bold());
                        std::process::exit(1);
                    }

                    println!("Workspace actualizado correctamente a: {}\n", chosen_path.display().to_string().cyan());
                    println!("{}", "Nota: Los proyectos existentes no han sido movidos automáticamente.".yellow());
                }
            }
        }
        Commands::Select | Commands::Guia => {
            use std::io::{self, Write};
            use colored::Colorize;

            'main_select: loop {
                println!("\n{}", "Selección de Proyecto de Hardware".bold());
                println!("¿Dónde desea buscar proyectos?\n");
                println!("[1] Workspace de ITO");
                println!("[2] Directorio actual de la terminal");
                println!("[3] Salir\n");

                let explore_path = loop {
                    print!("Seleccione una opción: ");
                    io::stdout().flush().ok();
                    let mut option = String::new();
                    if io::stdin().read_line(&mut option).is_err() {
                        println!("{}", "Error al leer la entrada.".red());
                        std::process::exit(1);
                    }
                    let option = option.trim();
                    if option == "1" {
                        match ito::load_workspace_config() {
                            Ok(Some(cfg)) => {
                                break std::path::PathBuf::from(&cfg.workspace).join("Projects");
                            }
                            Ok(None) => {
                                println!("{}", "No hay ningún Workspace configurado actualmente.".yellow());
                                println!("Configúrelo primero con 'ito workspace set' o corra 'ito new <Nombre>'.");
                                std::process::exit(1);
                            }
                            Err(err) => {
                                println!("{}", format!("❌ Error: {}", err).red().bold());
                                std::process::exit(1);
                            }
                        }
                    } else if option == "2" {
                        match std::env::current_dir() {
                            Ok(dir) => break dir,
                            Err(err) => {
                                println!("{}", format!("Error al obtener directorio actual: {}", err).red());
                                std::process::exit(1);
                            }
                        }
                    } else if option == "3" {
                        std::process::exit(0);
                    } else {
                        println!("{}", "Opción inválida. Intente de nuevo.".yellow());
                    }
                };

                'outer_selection: loop {
                    let projects = ito::scan_directory_for_projects(&explore_path);
                    if projects.is_empty() {
                        println!("{}", "\nNo se encontraron proyectos de ITO en la ubicación seleccionada.".yellow());
                        println!("¿Qué desea hacer?\n");
                        println!("[1] Clonar un proyecto existente desde la web (ito clone)");
                        println!("[2] Crear un nuevo proyecto local (ito new)");
                        println!("[3] Volver al menú de búsqueda");
                        println!("[4] Salir\n");

                        print!("Seleccione una opción: ");
                        io::stdout().flush().ok();
                        let mut choice = String::new();
                        if io::stdin().read_line(&mut choice).is_err() {
                            std::process::exit(1);
                        }
                        match choice.trim() {
                            "1" => {
                                print!("Ingrese el Token de API del proyecto, o su ID/nombre/URL (si ya inició sesión): ");
                                io::stdout().flush().ok();
                                let mut token_input = String::new();
                                if io::stdin().read_line(&mut token_input).is_ok() {
                                    let token_trimmed = token_input.trim().to_string();
                                    if !token_trimmed.is_empty() {
                                        // Cambiar temporalmente el directorio para clonar dentro de la carpeta explorada
                                        let prev_dir = std::env::current_dir().unwrap_or(std::path::PathBuf::from("."));
                                        let _ = std::env::set_current_dir(&explore_path);
                                        match ito::run_clone(token_trimmed).await {
                                            Ok(msg) => println!("\n{} {}", "OK".green().bold(), msg),
                                            Err(e) => println!("\n{} Error: {}", "ERROR".red().bold(), e),
                                        }
                                        let _ = std::env::set_current_dir(&prev_dir);
                                    }
                                }
                            }
                            "2" => {
                                print!("Ingrese el nombre del nuevo proyecto: ");
                                io::stdout().flush().ok();
                                let mut name_input = String::new();
                                if io::stdin().read_line(&mut name_input).is_ok() {
                                    let name_trimmed = name_input.trim().to_string();
                                    if !name_trimmed.is_empty() {
                                        match ito::run_new(explore_path.clone(), &name_trimmed) {
                                            Ok((_, msg)) => println!("\n{} {}", "OK".green().bold(), msg),
                                            Err(e) => println!("\n{} Error: {}", "ERROR".red().bold(), e),
                                        }
                                    }
                                }
                            }
                            "3" => {
                                continue 'main_select;
                            }
                            _ => std::process::exit(0),
                        }
                        continue 'outer_selection;
                    }

                    println!("\nProyectos encontrados:");
                    for (idx, proj) in projects.iter().enumerate() {
                        println!("  [{}] {} ({})", (idx + 1).to_string().cyan().bold(), proj.name.bold(), proj.path.display().to_string().dimmed());
                    }
                    println!("\nOpciones de creación:");
                    println!("  [{}] Clonar un proyecto desde la web (ito clone)", "c".cyan().bold());
                    println!("  [{}] Crear un nuevo proyecto local (ito new)", "n".cyan().bold());
                    println!("  [{}] Volver al menú de búsqueda", "v".cyan().bold());
                    println!("  [{}] Salir", "q".cyan().bold());
                    println!("");

                    print!("Seleccione una opción: ");
                    io::stdout().flush().ok();
                    let mut selection_input = String::new();
                    if io::stdin().read_line(&mut selection_input).is_err() {
                        println!("{}", "Error al leer la selección.".red());
                        std::process::exit(1);
                    }
                    let selection_input = selection_input.trim();
                    if selection_input.to_lowercase() == "q" {
                        std::process::exit(0);
                    }
                    if selection_input.to_lowercase() == "v" {
                        continue 'main_select;
                    }
                    if selection_input.to_lowercase() == "c" {
                        print!("Ingrese el Token de API del proyecto, o su ID/nombre/URL (si ya inició sesión): ");
                        io::stdout().flush().ok();
                        let mut token_input = String::new();
                        if io::stdin().read_line(&mut token_input).is_ok() {
                            let token_trimmed = token_input.trim().to_string();
                            if !token_trimmed.is_empty() {
                                let prev_dir = std::env::current_dir().unwrap_or(std::path::PathBuf::from("."));
                                let _ = std::env::set_current_dir(&explore_path);
                                match ito::run_clone(token_trimmed).await {
                                    Ok(msg) => println!("\n{} {}", "OK".green().bold(), msg),
                                    Err(e) => println!("\n{} Error: {}", "ERROR".red().bold(), e),
                                }
                                let _ = std::env::set_current_dir(&prev_dir);
                            }
                        }
                        continue 'outer_selection;
                    }
                    if selection_input.to_lowercase() == "n" {
                        print!("Ingrese el nombre del nuevo proyecto: ");
                        io::stdout().flush().ok();
                        let mut name_input = String::new();
                        if io::stdin().read_line(&mut name_input).is_ok() {
                            let name_trimmed = name_input.trim().to_string();
                            if !name_trimmed.is_empty() {
                                match ito::run_new(explore_path.clone(), &name_trimmed) {
                                    Ok((_, msg)) => println!("\n{} {}", "OK".green().bold(), msg),
                                    Err(e) => println!("\n{} Error: {}", "ERROR".red().bold(), e),
                                }
                            }
                        }
                        continue 'outer_selection;
                    }

                    if let Ok(idx) = selection_input.parse::<usize>() {
                        if idx > 0 && idx <= projects.len() {
                            let chosen_project = &projects[idx - 1];

                            let cd_command = format!("cd \"{}\"", chosen_project.path.display());
                            ito::copy_to_clipboard(&cd_command);
                            ito::write_goto_script(&cd_command);
                            let _ = ito::install_shell_wrappers();

                            loop {
                                println!("\n{}", "============================================================".dimmed());
                                println!("{} {}", "Gestión de Proyecto:".bold(), chosen_project.name.cyan().bold());
                                println!("{}\n", chosen_project.path.display().to_string().dimmed());
                                println!("[1] Entrar al proyecto (copia el comando 'cd' de navegación y sale)");
                                println!("[2] Descargar últimos cambios del servidor (ito pull)");
                                println!("[3] Subir cambios locales al servidor (ito push)");
                                println!("[4] Ver estado de enlaces y módulos (ito links)");
                                println!("[5] Vincular un módulo físico (ito link)");
                                println!("[6] Ver historial de versiones (ito history)");
                                println!("[7] Volver a la lista de proyectos");
                                println!("[8] Salir del asistente\n");

                                print!("Seleccione una opción: ");
                                io::stdout().flush().ok();
                                let mut menu_input = String::new();
                                if io::stdin().read_line(&mut menu_input).is_err() {
                                    println!("{}", "Error al leer la opción.".red());
                                    std::process::exit(1);
                                }

                                match menu_input.trim() {
                                    "1" => {
                                        println!("\nNavegación automática ejecutada (abre una nueva terminal para activar el autocompletado si no se actualizó de inmediato).");
                                        println!("(Comando cd copiado al portapapeles como respaldo)");
                                        std::process::exit(0);
                                    }
                                    "2" => {
                                        println!("\nDescargando versión completa desde el servidor...");
                                        match ito::run_pull(chosen_project.path.clone()).await {
                                            Ok(msg) => println!("\n{} {}", "OK".green().bold(), msg),
                                            Err(e) => println!("\n{} Error: {}", "ERROR".red().bold(), e),
                                        }
                                    }
                                    "3" => {
                                        println!("\nEnviando última versión al servidor...");
                                        match ito::run_push(chosen_project.path.clone()).await {
                                            Ok(msg) => println!("\n{} Sincronización exitosa: {}", "OK".green().bold(), msg),
                                            Err(e) => println!("\n{} Error: {}", "ERROR".red().bold(), e),
                                        }
                                    }
                                    "4" => {
                                        let ito_json_path = chosen_project.path.join("ito.json");
                                        if !ito_json_path.exists() {
                                            println!("\n{} No se encontró el archivo ito.json en el proyecto.", "Error:".red().bold());
                                        } else if let Ok(content) = std::fs::read_to_string(&ito_json_path) {
                                            if let Ok(config) = serde_json::from_str::<ito::models::ItoProjectConfig>(&content) {
                                                println!("\nMódulos vinculados en {}:", config.project_name.cyan().bold());
                                                let modules = [
                                                    ("firmware", "Firmware"),
                                                    ("electronics", "Electrónica"),
                                                    ("mechanical", "Mecánica"),
                                                    ("documentation", "Documentación"),
                                                    ("manufacturing", "Manufactura"),
                                                ];
                                                let links_map = config.links.unwrap_or_default();
                                                for (key, name) in &modules {
                                                    println!("\n{}", name.bold());
                                                    if let Some(link) = links_map.get(*key) {
                                                        println!("  {}", "Vinculado".green().bold());
                                                        println!("  Herramienta: {}", link.tool.cyan());
                                                        println!("  Motor:       {}", link.engine.yellow());
                                                        println!("  Ruta:        {}", link.path.dimmed());
                                                    } else {
                                                        println!("  {}", "No vinculado".red());
                                                    }
                                                }
                                            } else {
                                                println!("\n{} Error al parsear ito.json.", "Error:".red().bold());
                                            }
                                        } else {
                                            println!("\n{} Error al leer ito.json.", "Error:".red().bold());
                                        }
                                    }
                                    "5" => {
                                        println!("\nVincular Módulo Externo en {}", chosen_project.name.cyan().bold());
                                        println!("¿Qué tipo de módulo desea vincular?\n");
                                        println!("[1] Firmware");
                                        println!("[2] Electrónica");
                                        println!("[3] Mecánica");
                                        println!("[4] Documentación");
                                        println!("[5] Manufactura");
                                        println!("[6] Volver al menú del proyecto\n");

                                        print!("Seleccione una opción: ");
                                        io::stdout().flush().ok();
                                        let mut link_opt = String::new();
                                        if io::stdin().read_line(&mut link_opt).is_err() {
                                            continue;
                                        }
                                        let (module_key, module_name) = match link_opt.trim() {
                                            "1" => ("firmware", "Firmware"),
                                            "2" => ("electronics", "Electrónica"),
                                            "3" => ("mechanical", "Mecánica"),
                                            "4" => ("documentation", "Documentación"),
                                            "5" => ("manufacturing", "Manufactura"),
                                            _ => continue,
                                        };

                                        println!("\nAbriendo explorador de Windows para seleccionar la carpeta...");
                                        let selected_path = ito::open_folder_dialog(&format!("Selecciona la carpeta de {}", module_name));
                                        let target_path = match selected_path {
                                            Some(path) => {
                                                println!("Carpeta seleccionada: {}", path.cyan().bold());
                                                ito::copy_to_clipboard(&path);
                                                std::path::PathBuf::from(path)
                                            }
                                            None => {
                                                println!("{}", "Warning: Diálogo cancelado. Ingrese la ruta manual:".yellow());
                                                print!("Ruta absoluta: ");
                                                io::stdout().flush().ok();
                                                let mut path_input = String::new();
                                                if io::stdin().read_line(&mut path_input).is_err() {
                                                    continue;
                                                }
                                                std::path::PathBuf::from(path_input.trim())
                                            }
                                        };

                                        match ito::run_link(chosen_project.path.clone(), module_key, target_path.clone()) {
                                            Ok(tool) => {
                                                if tool == "Unknown" {
                                                    println!("\nWarning: No se pudo identificar el software automáticamente.");
                                                } else {
                                                    println!("\nProyecto {} detectado.", tool.green().bold());
                                                }
                                                println!("Módulo {} vinculado correctamente a: {}", module_name.green().bold(), target_path.display().to_string().cyan());
                                            }
                                            Err(err) => {
                                                println!("\nError al vincular: {}", err.red().bold());
                                            }
                                        }
                                    }
                                    "6" => {
                                        let history_path = chosen_project.path.join(".ito").join("history.toml");
                                        if !history_path.exists() {
                                            println!("\n{}", "No hay ningún commit registrado en este repositorio todavía.".yellow());
                                            println!("Comienza guardando una versión con: {}", "ito commit -m \"Mensaje\"".cyan());
                                        } else if let Ok(content) = std::fs::read_to_string(&history_path) {
                                            let history: ito::History = toml::from_str(&content).unwrap_or_default();
                                            if history.commits.is_empty() {
                                                println!("\n{}", "No hay ningún commit registrado en este repositorio todavía.".yellow());
                                            } else {
                                                println!("\n{}", "Historial de Revisiones de Hardware".bold());
                                                println!("------------------------------------------------------------");
                                                for commit in history.commits.iter().rev() {
                                                    let short_hash = if commit.hash.len() > 8 { &commit.hash[..8] } else { &commit.hash };
                                                    println!("Commit:  {}", short_hash.cyan().bold());
                                                    println!("Fecha:   {}", commit.timestamp.dimmed());
                                                    println!("Mensaje: {}", commit.message.bold());
                                                    println!("------------------------------------------------------------");
                                                }
                                            }
                                        }
                                    }
                                    "7" => {
                                        break;
                                    }
                                    "8" => {
                                        println!("\n¡Hasta luego!");
                                        std::process::exit(0);
                                    }
                                    _ => {
                                        println!("{}", "Opción inválida. Intente de nuevo.".yellow());
                                    }
                                }
                            }
                        }
                    }
                    println!("{}", "Número inválido. Intente de nuevo.".yellow());
                }
            }
        }
        Commands::Link => {
            use std::io::{self, Write};
            use colored::Colorize;

            let current_dir = std::env::current_dir()?;
            let root = match ito::find_project_root(&current_dir) {
                Some(r) => r,
                None => {
                    println!("{}", "Error: No se encontró la raíz del proyecto. ¿Ejecutaste 'ito init' o 'ito new' primero?".red().bold());
                    std::process::exit(1);
                }
            };

            println!("{}", "Vincular Módulo Externo".bold());
            println!("¿Qué tipo de módulo desea vincular?\n");
            println!("[1] Firmware");
            println!("[2] Electrónica");
            println!("[3] Mecánica");
            println!("[4] Documentación");
            println!("[5] Manufactura");
            println!("[6] Volver atrás\n");

            let (module_key, module_name) = loop {
                print!("Seleccione una opción: ");
                io::stdout().flush().ok();
                let mut option = String::new();
                if io::stdin().read_line(&mut option).is_err() {
                    println!("{}", "Error al leer la opción.".red());
                    std::process::exit(1);
                }
                match option.trim() {
                    "1" => break ("firmware", "Firmware"),
                    "2" => break ("electronics", "Electrónica"),
                    "3" => break ("mechanical", "Mecánica"),
                    "4" => break ("documentation", "Documentación"),
                    "5" => break ("manufacturing", "Manufactura"),
                    "6" => {
                        println!("{}", "Vinculación cancelada.".yellow());
                        return Ok(());
                    }
                    _ => println!("{}", "Opción inválida. Intente de nuevo.".yellow()),
                }
            };

            println!("\nAbriendo explorador de Windows para seleccionar la carpeta del proyecto...");
            let selected_path = ito::open_folder_dialog(&format!("Selecciona la carpeta de {}", module_name));
            
            let target_path = match selected_path {
                Some(path) => {
                    println!("Carpeta seleccionada: {}", path.cyan().bold());
                    ito::copy_to_clipboard(&path);
                    println!("Ruta copiada al portapapeles automáticamente.");
                    std::path::PathBuf::from(path)
                }
                None => {
                    println!("{}", "Warning: Diálogo cancelado. Ingrese la ruta manual:".yellow());
                    print!("Ruta absoluta: ");
                    io::stdout().flush().ok();
                    let mut path_input = String::new();
                    if io::stdin().read_line(&mut path_input).is_err() {
                        println!("{}", "Error al leer la ruta.".red());
                        std::process::exit(1);
                    }
                    std::path::PathBuf::from(path_input.trim())
                }
            };

            match ito::run_link(root, module_key, target_path.clone()) {
                Ok(tool) => {
                    if tool == "Unknown" {
                        println!("\n{}", "Warning: No se pudo identificar automáticamente el software de desarrollo en la carpeta.".yellow());
                    } else {
                        println!("\nProyecto {} detectado.", tool.green().bold());
                    }
                    println!("Módulo {} vinculado correctamente a: {}\n", module_name.green().bold(), target_path.display().to_string().cyan());
                    println!("Note: Puedes auditar tus enlaces en cualquier momento con: {}", "ito links".cyan());
                    println!("Note: Si ya vinculaste tus módulos principales, verifica el estado de tu diseño con: {}", "ito status".cyan());
                }
                Err(err) => {
                    println!("{}", format!("Error: {}", err).red().bold());
                    std::process::exit(1);
                }
            }
        }
        Commands::Links => {
            use colored::Colorize;

            let current_dir = std::env::current_dir()?;
            let root = match ito::find_project_root(&current_dir) {
                Some(r) => r,
                None => {
                    println!("{}", "Error: No se encontró la raíz del proyecto. ¿Ejecutaste 'ito init' o 'ito new' primero?".red().bold());
                    std::process::exit(1);
                }
            };

            let ito_json_path = root.join("ito.json");
            if !ito_json_path.exists() {
                println!("{}", "Error: No se encontró el archivo ito.json en el proyecto actual.".red().bold());
                std::process::exit(1);
            }

            let content = match std::fs::read_to_string(&ito_json_path) {
                Ok(c) => c,
                Err(e) => {
                    println!("{}", format!("Error al leer ito.json: {}", e).red().bold());
                    std::process::exit(1);
                }
            };

            let config: ito::models::ItoProjectConfig = match serde_json::from_str(&content) {
                Ok(cfg) => cfg,
                Err(e) => {
                    println!("{}", format!("Error al parsear ito.json: {}", e).red().bold());
                    std::process::exit(1);
                }
            };

            println!("\n{}", "Proyecto actual".bold());
            println!("{}\n", config.project_name.cyan().bold());
            println!("{}", "Módulos vinculados".bold());

            let modules = [
                ("firmware", "Firmware"),
                ("electronics", "Electrónica"),
                ("mechanical", "Mecánica"),
                ("documentation", "Documentación"),
                ("manufacturing", "Manufactura"),
            ];

            let links_map = config.links.unwrap_or_default();

            for (key, name) in &modules {
                println!("\n{}", name.bold());
                if let Some(link) = links_map.get(*key) {
                    println!("  {}", "Vinculado".green().bold());
                    println!("  Herramienta: {}", link.tool.cyan());
                    println!("  Motor:       {}", link.engine.yellow());
                    println!("  Ruta:        {}", link.path.dimmed());
                } else {
                    println!("  {}", "No vinculado".red());
                }
            }
        }
        Commands::Go { module } => {
            use std::io::{self, Write};
            use colored::Colorize;

            let current_dir = std::env::current_dir()?;
            let root = match ito::find_project_root(&current_dir) {
                Some(r) => r,
                None => {
                    println!("{}", "Error: No se encontró ningún proyecto de Ito asociado.".red().bold());
                    std::process::exit(1);
                }
            };

            let ito_json_path = root.join("ito.json");
            if !ito_json_path.exists() {
                println!("{}", "Error: No se encontró el archivo ito.json en el proyecto.".red().bold());
                std::process::exit(1);
            }

            let content = std::fs::read_to_string(&ito_json_path)?;
            let config: ito::models::ItoProjectConfig = serde_json::from_str(&content)?;
            let links_map = config.links.unwrap_or_default();

            let modules = [
                ("firmware", "Firmware"),
                ("electronics", "Electrónica"),
                ("mechanical", "Mecánica"),
                ("documentation", "Documentación"),
                ("manufacturing", "Manufactura"),
            ];

            let (target_key, target_name) = match module {
                Some(ref m_arg) => {
                    let m_lower = m_arg.to_lowercase();
                    let matched = modules.iter().find(|(k, _)| *k == m_lower.as_str() || m_lower.starts_with(&k[..3]));
                    match matched {
                        Some((k, n)) => (k.to_string(), n.to_string()),
                        None => {
                            println!("{}", format!("Error: Módulo '{}' no válido. Use uno de: firmware, electronics, mechanical, documentation, manufacturing.", m_arg).red());
                            std::process::exit(1);
                        }
                    }
                }
                None => {
                    println!("{}", "Navegación de Módulos".bold());
                    println!("¿A qué módulo desea ir?\n");
                    for (idx, (key, name)) in modules.iter().enumerate() {
                        if let Some(link) = links_map.get(*key) {
                            println!("  [{}] {} ({})", (idx + 1).to_string().cyan().bold(), name.bold(), link.path.dimmed());
                        } else {
                            println!("  [{}] {} ({})", (idx + 1).to_string().dimmed(), name.dimmed(), "No vinculado".red());
                        }
                    }
                    println!("");
                    
                    loop {
                        print!("Seleccione una opción: ");
                        io::stdout().flush().ok();
                        let mut option = String::new();
                        if io::stdin().read_line(&mut option).is_err() {
                            println!("{}", "Error al leer la opción.".red());
                            std::process::exit(1);
                        }
                        let option = option.trim();
                        if let Ok(idx) = option.parse::<usize>() {
                            if idx > 0 && idx <= modules.len() {
                                let (k, n) = modules[idx - 1];
                                break (k.to_string(), n.to_string());
                            }
                        }
                        println!("{}", "Opción inválida. Intente de nuevo.".yellow());
                    }
                }
            };

            if let Some(link) = links_map.get(&target_key) {
                let cd_command = format!("cd \"{}\"", link.path);
                ito::copy_to_clipboard(&cd_command);
                ito::write_goto_script(&cd_command);
                let _ = ito::install_shell_wrappers();
                println!("\nMódulo seleccionado: {}", target_name.cyan().bold());
                println!("Ruta: {}", link.path.cyan());
                println!("\nNavegación automática ejecutada.");
                println!("(Comando cd copiado al portapapeles como respaldo)");
            } else {
                println!("\nEl módulo {} no está vinculado todavía.", target_name.red().bold());
                println!("Note: Vincúlalo primero con: {}", format!("ito link").cyan());
            }
        }
        Commands::Auth { subcommand } => {
            match subcommand {
                AuthSubcommand::Login { token } => {
                    let current_dir = std::env::current_dir()?;
                    let project_root = ito::find_project_root(&current_dir).unwrap_or(current_dir.clone());
                    match ito::run_auth_login(project_root, token) {
                        Ok(_) => {
                            println!("{}", "Autenticación exitosa. El token ha sido guardado para este proyecto.".green().bold());
                        }
                        Err(e) => {
                            anyhow::bail!("{}", e);
                        }
                    }
                }
            }
        }
        Commands::Push => {
            let current_dir = std::env::current_dir()?;
            let project_root = ito::find_project_root(&current_dir).unwrap_or(current_dir.clone());
            match ito::run_push(project_root).await {
                Ok(msg) => {
                    println!("{} Sincronización exitosa: {}", "OK".green().bold(), msg);
                }
                Err(e) => {
                    anyhow::bail!("{}", e);
                }
            }
        }
        Commands::Pull => {
            let current_dir = std::env::current_dir()?;
            let project_root = ito::find_project_root(&current_dir).unwrap_or(current_dir.clone());
            match ito::run_pull(project_root).await {
                Ok(msg) => {
                    println!("{} Descarga y restauración exitosa: {}", "OK".green().bold(), msg);
                }
                Err(e) => {
                    anyhow::bail!("{}", e);
                }
            }
        }
        Commands::Clone { token_or_project } => {
            match ito::run_clone(token_or_project.clone()).await {
                Ok(msg) => {
                    println!("{} {}", "OK".green().bold(), msg);
                }
                Err(e) => {
                    anyhow::bail!("{}", e);
                }
            }
        }
        Commands::Login | Commands::Logueo => {
            use std::io::{self, Write};
            use colored::Colorize;

            println!("{}", "Inicio de Sesión en ITOGravity".bold());
            print!("Correo Electrónico: ");
            io::stdout().flush().ok();
            let mut email = String::new();
            if io::stdin().read_line(&mut email).is_err() {
                anyhow::bail!("Error al leer el correo electrónico.");
            }
            let email = email.trim();

            print!("Contraseña: ");
            io::stdout().flush().ok();
            
            let mut password = String::new();
            if io::stdin().read_line(&mut password).is_err() {
                anyhow::bail!("Error al leer la contraseña.");
            }
            let password = password.trim().to_string();

            if email.is_empty() || password.is_empty() {
                anyhow::bail!("El correo electrónico y la contraseña son obligatorios.");
            }

            match ito::run_login(email, &password).await {
                Ok(msg) => {
                    println!("{} {}", "OK".green().bold(), msg);
                }
                Err(e) => {
                    anyhow::bail!("{}", e);
                }
            }
        }
        Commands::Update { force } => {
            println!("Comprobando actualizaciones en GitHub...");
            match ito::updater::check_for_updates(*force).await {
                Ok(Some(new_version)) => {
                    println!("Actualización disponible: v{}.", new_version.trim_start_matches('v'));
                    if let Err(e) = ito::updater::download_and_install_update(&new_version).await {
                        anyhow::bail!("Error al instalar la actualización: {}", e);
                    }
                }
                Ok(None) => {
                    println!("¡Ya estás usando la versión más reciente de ITO (v{})!", env!("CARGO_PKG_VERSION"));
                }
                Err(e) => {
                    anyhow::bail!("Error al comprobar actualizaciones: {}", e);
                }
            }
        }
    }

    Ok(())
}
