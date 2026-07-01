# ITO (糸) — Motor de Versionado Semántico y Almacenamiento CAS para Ingeniería de Hardware

ITO es una herramienta de línea de comandos (CLI) de nivel industrial y de código abierto diseñada bajo la plataforma Alexandria HQ para la administración unificada del ciclo de vida y control de versiones en proyectos de ingeniería de hardware y sistemas embebidos.

A diferencia de los sistemas de control de versiones tradicionales como Git, que analizan diferencias por línea de texto plano y tratan los archivos de diseño CAD/3D como cajas negras binarias densas, ITO entiende la semántica física y eléctrica detrás de cada plano, lista de materiales (BOM) y asignación de pines de microcontroladores.

---

## 1. Arquitectura y Componentes Clave

```
                         [ ITO CLI (Interfaz Unificada) ]
                                        │
             ┌──────────────────────────┼──────────────────────────┐
             ▼                          ▼                          ▼
      [ GitEngine ]            [ SemanticCadEngine ]      [ FileHashEngine ]
    (Módulo Firmware)          (Módulo Electrónica)      (Módulos Mecánica/Doc)
             │                          │                          │
             └──────────────────────────┼──────────────────────────┘
                                        ▼
                         [ Almacén de Objetos CAS ]
                          (.ito/objects/ SHA-256)
                                        │
                                        ▼
                         [ Historial Transaccional ]
                             (.ito/history.toml)
```

### Motores de Versionado Semántico Integrados
*   **`GitEngine` (Firmware)**: Enlaza de manera inteligente el repositorio Git nativo de tu firmware a la transacción de ITO, registrando el hash del commit actual en los metadatos de ITO sin duplicar el código.
*   **`SemanticCadEngine` (Electrónica)**: Extrae de forma semántica la Netlist de circuitos integrados (KiCad, Altium, Eagle) para reportar diferencias lógicas en componentes (cambios de footprints, valores) y conexiones (nets), omitiendo diferencias de visualización del editor.
*   **`FileHashEngine` (Mecánica, Documentación, Manufactura)**: Motor optimizado para procesar árboles completos de planos mecánicos (STEP, SolidWorks), PDFs y archivos de fabricación pesados.

### Almacenamiento CAS (Content-Addressable Storage) con Deduplicación
Para optimizar el almacenamiento local y la sincronización con servidores, ITO implementa una base de datos direccionable por contenido:
*   Cada archivo se identifica mediante el hash **SHA-256** de su contenido.
*   Los objetos se guardan estructurados en `.ito/objects/`.
*   **Deduplicación Total**: Si múltiples módulos o versiones sucesivas contienen archivos idénticos (como modelos 3D pesados), estos se guardan una única vez física en disco, referenciándose mediante manifiestos de proyecto.

### Motor de Exclusiones Inteligente (`.itoignore`)
Filtrado automático de archivos y carpetas pesadas/temporales de compilación para resguardar únicamente el diseño fuente de la disciplina:
*   **Exclusiones por defecto**: Directorios de compilación y dependencias (`.git`, `.ito`, `.pio`, `target`, `node_modules`, `.venv`, `bin`, `obj`, `.vs`, `history`), bloqueos de CAD (`.lck`), archivos temporales de Office/SolidWorks (`~$*`, `.~*`), `*.tmp` y respaldos `*.bak`.

---

## 2. Referencia de Comandos CLI

El uso de la CLI está estructurado con base en estándares de comandos de Git y herramientas de desarrollo modernas.

| Comando | Descripción |
| :--- | :--- |
| `ito init` | Inicializa un repositorio de ITO en el directorio actual, creando la estructura `.ito/`. |
| `ito new <nombre>` | Crea un proyecto estructurado y normalizado para ingeniería multidisciplinar en el Workspace. |
| `ito status` | Audita y reporta el estado actual de todos los módulos vinculados (BOM, CAD, Firmware). |
| `ito diff` | Muestra diferencias semánticas detalladas de componentes y redes (nets) contra la última caché. |
| `ito commit [-m "msg"]` | Ejecuta el linter eléctrico (ERC) y guarda una instantánea inmutable en el historial y el CAS. |
| `ito log` | Muestra el historial completo de commits del proyecto y el resumen de cambios numéricos. |
| `ito restore <hash>` | Restaura el estado de todos los módulos vinculados al commit especificado desde el CAS. |
| `ito lint` | Ejecuta de forma estática reglas de diseño eléctrico semántico (ERC) sobre los módulos de electrónica. |
| `ito workspace` | Muestra la ruta física del Workspace global y el conteo de proyectos registrados. |
| `ito workspace set [ruta]` | Configura o cambia el directorio de trabajo del Workspace global. |
| `ito select` | Menú interactivo de selección de proyectos que inyecta navegación automática a la consola activa. |
| `ito link` | Enlaza un directorio físico externo mediante un explorador de carpetas visual de Windows. |
| `ito links` | Lista todos los enlaces configurados en el proyecto indicando su motor y herramienta. |
| `ito go <módulo>` | Navega de forma automática a la ruta del módulo seleccionado (firmware, electronics, etc.). |

---

## 3. Linter Eléctrico: Reglas de Diseño Semántico (ERC)

El comando `ito lint` realiza auditorías de integridad de circuitos buscando las siguientes fallas comunes:
1.  **Entradas Flotantes (Floating Inputs)**: Pines de tipo entrada que carecen de conexión a una señal o red eléctrica.
2.  **Cortocircuitos de Salidas (Output Short-Circuits)**: Dos o más pines de tipo salida configurados directamente en la misma red de circuito.
3.  **Redes Huérfanas (Single-pin Nets)**: Redes eléctricas conectadas a un único pin en todo el esquemático.

---

## 4. Desarrollo Local

### Prerrequisitos
*   Rust (edición 2021) y `cargo`.
*   PowerShell en Windows (para la inyección de wrappers de navegación).

### Compilación y Testeo
```bash
# Comprobar la compilación de la biblioteca y ejecutable
cargo check

# Ejecutar la suite de pruebas unitarias e integrales (22 tests)
cargo test

# Instalar globalmente el ejecutable de ITO en el sistema
cargo install --path . --force
```
