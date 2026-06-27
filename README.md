# Ito (糸) - El Hilo Rojo del Hardware

> *"Un hilo rojo invisible conecta a aquellos que están destinados a encontrarse, sin importar el tiempo, el lugar o las circunstancias. El hilo se puede estirar o contraer, pero nunca romper."*

**Ito** es un motor de control de versiones semántico y de código abierto diseñado específicamente para la ingeniería de hardware. A diferencia de los sistemas de control de versiones tradicionales como Git (que analizan diferencias de líneas de texto plano o tratan a los archivos CAD como cajas negras binarias), **Ito** entiende el significado eléctrico, físico y de fabricación detrás de cada cambio en tu diseño.

Conecta de forma semántica los tres pilares del desarrollo de hardware que hoy viven fragmentados:
1. **La Lista de Materiales (BOM)**: Componentes, fabricantes, MPNs, empaquetado y costos.
2. **El Diseño Esquemático y PCB (CAD)**: Conectividad eléctrica (nets), asignación de pines, footprints y ruteado.
3. **El Código de Firmware**: La asignación y configuración de pines de entrada/salida (I/O) en los microcontroladores.

---

## 🚀 Características Clave (En Desarrollo)

- **Versionado Semántico Real**: Detecta cuando un cambio es puramente estético (ej. mover un componente de lugar en el esquemático) vs. cuando altera la conectividad eléctrica (ej. un pin flotante o un cambio de pista).
- **Consistencia Interdisciplinaria**: Asegura que un cambio en el MPN de la BOM mantenga coherencia con el footprint en la PCB y los registros I/O del firmware.
- **Diferenciación Inteligente (Semantic Diff)**: Reporta cambios claros y legibles para humanos (ej. *"Resistencia R1 cambiada de 10k a 4.7k"* o *"Pin 3 de U1 conectado a la red SPI_CS"*).
- **Diseñado en Rust**: Rápido, seguro en memoria y con binarios estáticos ultraligeros y sin dependencias pesadas.

---

## 🛠️ Interfaz de Línea de Comandos (CLI)

Ito se expone mediante un CLI intuitivo y familiar para usuarios de Git:

```bash
# Inicializar un nuevo repositorio Ito
ito init

# Ver el estado del área de trabajo (BOM, CAD, Firmware)
ito status

# Comparar cambios semánticos detallados
ito diff
```

---

## 🏗️ Estructura del Proyecto

- `src/main.rs`: Punto de entrada de la aplicación y parsing de argumentos CLI utilizando `clap`.
- `src/models.rs`: Definición de las estructuras de datos núcleo (`Component`, `Pin`, `Net`, `HardwareDesign`) que representan el modelo eléctrico y físico del hardware.

---

## ⚙️ Desarrollo Local

Para compilar y correr las pruebas del motor Ito:

```bash
# Comprobar compilación
cargo check

# Ejecutar las pruebas unitarias
cargo test

# Ejecutar la CLI de desarrollo
cargo run -- --help
```
