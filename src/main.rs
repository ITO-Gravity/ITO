mod models;
mod parsers;

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
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match &cli.command {
        Commands::Init => {
            println!("Inicializando repositorio Ito en el directorio actual...");
            // TODO: Crear directorio .ito, estructuras de control, etc.
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
        Commands::Diff { path } => {
            if let Some(p) = path {
                println!("Calculando diferencias semánticas para: {}", p);
            } else {
                println!("Calculando diferencias semánticas globales...");
            }
            // TODO: Implementar lógica de diferencias semánticas.
        }
    }

    Ok(())
}
