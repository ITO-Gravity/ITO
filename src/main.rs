mod models;

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
            println!("Analizando estado semántico del hardware...");
            
            // Ejemplo de uso del modelo semántico
            let mut _design = models::HardwareDesign::new();
            
            // TODO: Escanear BOM, CAD/esquemas, y Firmware para buscar mutaciones.
            println!("  BOM: Sin cambios detectados.");
            println!("  CAD: Sin cambios detectados.");
            println!("  Firmware: Sin cambios detectados.");
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
