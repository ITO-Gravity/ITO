# ITO (糸) — Motor de Versionado Semántico y Almacenamiento CAS para Ingeniería de Hardware

ITO es una herramienta de línea de comandos (CLI) de nivel industrial y de código abierto diseñada bajo la plataforma ITO Gravity para la administración unificada del ciclo de vida y control de versiones en proyectos de ingeniería de hardware y sistemas embebidos.

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
*   **`SemanticCadEngine` (Electrónica)**: Extrae de forma semántica el diseño de circuitos (KiCad, Eagle, EDIF y Proteus) para reportar diferencias lógicas en componentes (cambios de footprints, valores) y conexiones (nets), omitiendo diferencias de visualización del editor. El soporte de **Proteus** (`.pdsprj`) lee componentes, valores y footprints directamente del proyecto, sin exportar netlist a mano (extracción de nets en desarrollo).
*   **`FileHashEngine` (Mecánica, Documentación, Manufactura)**: Motor optimizado para procesar árboles completos de planos mecánicos (STEP, SolidWorks), PDFs y archivos de fabricación pesados.

### Almacenamiento CAS (Content-Addressable Storage) con Deduplicación
Para optimizar el almacenamiento local y la sincronización con servidores, ITO implementa una base de datos direccionable por contenido:
*   Cada archivo se identifica mediante el hash **SHA-256** de su contenido.
*   Los objetos se guardan estructurados en `.ito/objects/`.
*   **Deduplicación Total**: Si múltiples módulos o versiones sucesivas contienen archivos idénticos (como modelos 3D pesados), estos se guardan una única vez física en disco, referenciándose mediante manifiestos de proyecto.

### Integridad del Historial y Restauración Segura
El motor está diseñado para que cada versión sea verificable y las restauraciones no pierdan trabajo:
*   **Identidad por contenido (árbol Merkle)**: El ID de cada commit se deriva de un hash del *contenido real* de todos los módulos (más el commit padre, mensaje y timestamp), no de un resumen de texto. Esto hace la detección de cambios fiable (sin falsos "no hay cambios") y las versiones reproducibles y resistentes a colisiones.
*   **Restauración atómica y no destructiva**: `ito restore` verifica que todos los objetos existan en el CAS antes de tocar el disco, restaura archivo por archivo de forma atómica (temporal + `rename`) y **nunca elimina archivos no rastreados** del usuario.
*   **Escritura atómica de metadatos**: El historial (`.ito/history.toml`) se escribe con la técnica temporal + `rename` para no corromperse ante un corte inesperado.

### Motor de Exclusiones Inteligente (`.itoignore`)
Filtrado automático de archivos y carpetas pesadas/temporales de compilación para resguardar únicamente el diseño fuente de la disciplina:
*   **Exclusiones por defecto**: Directorios de compilación y dependencias (`.git`, `.ito`, `.pio`, `target`, `node_modules`, `.venv`, `bin`, `obj`, `.vs`, `history`), bloqueos de CAD (`.lck`), archivos temporales de Office/SolidWorks (`~$*`, `.~*`), `*.tmp`, respaldos `*.bak`, y respaldos/estado de sesión de Proteus (`Project Backups/`, `*.pdsbak`, `*.workspace`).

---

## 2. Referencia de Comandos CLI

El uso de la CLI está estructurado con base en estándares de comandos de Git y herramientas de desarrollo modernas.

| Comando | Descripción |
| :--- | :--- |
| `ito init` | Inicializa un repositorio de ITO en el directorio actual, creando la estructura `.ito/`. |
| `ito new <nombre>` | Crea un proyecto estructurado y normalizado para ingeniería multidisciplinar en el Workspace. |
| `ito folder <nombre>` | Crea una carpeta personalizada en la raíz del proyecto y la registra como un módulo más, versionada junto al resto (alias: `ito carpeta`; `--list` para listarlas). |
| `ito status` | Audita y reporta el estado actual de todos los módulos vinculados (BOM, CAD, Firmware). |
| `ito diff` | Muestra diferencias semánticas detalladas de componentes y redes (nets) contra la última caché. |
| `ito commit [-m "msg"]` | Ejecuta el linter eléctrico (ERC) y guarda una instantánea inmutable en el historial y el CAS. |
| `ito log` | Muestra el historial completo de commits del proyecto y el resumen de cambios numéricos. |
| `ito restore <hash>` | Restaura el estado de todos los módulos vinculados al commit especificado desde el CAS. |
| `ito lint` | Ejecuta de forma estática reglas de diseño eléctrico semántico (ERC) sobre los módulos de electrónica. |
| `ito workspace` | Muestra la ruta física del Workspace global y el conteo de proyectos registrados. |
| `ito workspace set [ruta]` | Configura o cambia el directorio de trabajo del Workspace global. |
| `ito select` | Menú interactivo de selección de proyectos que inyecta navegación automática a la consola activa. |
| `ito guia` | Inicia el asistente interactivo para guiar al operador paso a paso (alias de `select`). |
| `ito link` | Enlaza un directorio físico externo mediante un explorador de carpetas visual de Windows. |
| `ito links` | Lista todos los enlaces configurados en el proyecto indicando su motor y herramienta. |
| `ito go <módulo>` | Copia al portapapeles la instrucción para navegar a un módulo enlazado (firmware, electronics, etc.). |
| `ito login` | Inicia sesión con tus credenciales de ITO Gravity (Email y Contraseña). |
| `ito clone <token>` | Clona un proyecto existente desde el servidor remoto de ITO Gravity. |
| `ito push` | Envía la última versión local del proyecto al servidor remoto de ITO Gravity. |
| `ito pull` | Descarga la última versión registrada del proyecto desde el servidor remoto de ITO Gravity. |
| `ito update` | Comprueba y actualiza ITO a la última versión disponible en GitHub de forma manual. |

---

## 3. Linter Eléctrico: Reglas de Diseño Semántico (ERC)

El comando `ito lint` realiza auditorías de integridad de circuitos con las siguientes reglas (código y severidad):
1.  **`E001_FLOATING_INPUT`** (Advertencia): Pines de tipo entrada (`Input`) sin conexión a ninguna red eléctrica.
2.  **`E002_NO_DRIVER_NET`** (Advertencia): Redes con entradas conectadas pero sin ningún emisor de señal (salida, pasivo o bidireccional) que las alimente.
3.  **`E003_UNCONNECTED_POWER`** (Advertencia): Pines de alimentación (`PowerInput`) sin conectar en circuitos integrados.
4.  **`E004_OUTPUT_SHORT`** (Crítico): Dos o más pines de salida conectados en la misma red, provocando un cortocircuito por conflicto de salidas.

---

## 4. Actualizaciones Automáticas

ITO cuenta con un actualizador automático integrado conectado a GitHub Releases:
- **Comprobación de fondo silenciosa**: Cada 24 horas, ITO realiza una consulta rápida (con un límite de 3 segundos de timeout) al iniciar cualquier comando ordinario para comprobar si existe una versión más nueva en GitHub de forma silenciosa.
- **Actualización manual**: Puedes forzar la comprobación y actualización en cualquier momento utilizando el comando `ito update` o `ito update --force`.
- **Soporte para repositorios privados**: Si el repositorio de ITO es privado, puedes configurar la variable de entorno `GITHUB_TOKEN` o `GH_TOKEN` en tu sistema para autenticar las peticiones del actualizador.

---

## 5. Desarrollo Local

### Prerrequisitos
*   Rust (edición 2021) y `cargo`.
*   PowerShell en Windows (para la inyección de wrappers de navegación).

### Compilación y Testeo
```bash
# Comprobar la compilación de la biblioteca y ejecutable
cargo check

# Ejecutar la suite de pruebas unitarias e integrales (31 tests)
cargo test

# Instalar globalmente el ejecutable de ITO en el sistema
cargo install --path . --force
```
