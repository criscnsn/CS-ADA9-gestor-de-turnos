# Reporte de Benchmark de Rendimiento con 200 Turnos en SQLite (`RNF-P01`)

> **Sub-issue:** `#07-B` (GitHub `#17`, parte de `#15`) · **Rama:** `feature/7b-benchmark-200-turnos`
> **Requisito verificado:** `RNF-P01` (*Tiempo de respuesta ≤ 2 segundos con al menos 200 turnos registrados*)
> **Suite ejecutable:** [`src-tauri/tests/benchmark_200_turnos.rs`](../src-tauri/tests/benchmark_200_turnos.rs)

---

## 1. Objetivo

Verificar de manera automatizada y reproducible que el coordinador real de producción (`comandos::Sgt` + `repository::RepositorioSqlite` vía `comandos::iniciar_sgt_en_ruta`) responde en **≤ 2 segundos (`2 000 ms`)** por operación cuando la base de datos local **SQLite en disco** almacena un volumen base de **200 turnos**:

1. **Restauración desde SQLite** (`iniciar_sgt_en_ruta` / `Repositorio::cargar` — `RF-11`, `RD-03`)
2. **Consulta de estado y fila FIFO** (`Sgt::obtener_estado` — `RF-03`, `RN-02`)
3. **Registro de nuevo turno** (`SistemaTurnos::registrar_turno` vía `Sgt::ejecutar` — `RF-02`, `RN-01`, `RN-02`)
4. **Asignación del siguiente turno a ventanilla** (`SistemaTurnos::asignar_siguiente` vía `Sgt::ejecutar` — `RF-04`)
5. **Finalización de atención en ventanilla** (`SistemaTurnos::finalizar_atencion` vía `Sgt::ejecutar` — `RF-06`, `RN-08`)

---

## 2. Especificaciones del Equipo de Prueba

| Componente | Especificación |
| :--- | :--- |
| **Procesador (CPU)** | 12th Gen Intel(R) Core(TM) i7-12700H (1 socket, 20 hilos lógicos) |
| **Memoria RAM** | ~7.77 GB (`7 766 812 kB` MemTotal) |
| **Sistema Operativo** | Fedora Linux 43 (Workstation Edition) · Kernel `7.3.4-100.fc43.x86_64` |
| **Almacenamiento** | Disco local (`std::env::temp_dir()`) con SQLite `PRAGMA journal_mode = WAL` y `synchronous = NORMAL` |
| **Toolchain Rust** | `rustc 1.100.0` (`stable-x86_64-unknown-linux-gnu`), perfil `test` (`unoptimized + debuginfo`) |
| **Motor de Base de Datos** | SQLite 3 embebido vía `rusqlite 0.32.1` (`features = ["bundled"]`) |

---

## 3. Metodología y Aislamiento de Datos

1. **Aislamiento total de la BD del usuario:**
   * Cada ejecución crea un directorio único temporal dentro de `std::env::temp_dir()` mediante el guardián RAII `BaseDatosTemporal` (`sgt_bench_7b_<etiqueta>_<pid>_<nanos>/sgt.db`), inicializado con `gestor_de_turnos_lib::db::abrir_conexion`.
   * Al finalizar la prueba (incluso ante un fallo), `Drop` elimina el directorio temporal completo sin tocar `app_data_dir()`.
2. **Dataset realista precargado de 200 turnos (`T-0001` a `T-0200`):**
   * `1..=100` (**100 turnos**): estado `ATENDIDO`, distribuidos entre `V01`, `V02` y `V03`.
   * `101..=120` (**20 turnos**): estado `CANCELADO` (`ventanilla_asignada = NULL`).
   * `121` (**1 turno**): estado `EN_ATENCION` activo en `V02` (`ventanillas.turno_actual_id = 'T-0121'`), dejando `V01` y `V03` libres y habilitadas.
   * `122..=200` (**79 turnos**): estado `EN_ESPERA` en la cola FIFO.
   * `secuencias('turno').ultimo_valor = 200`.
3. **Reapertura real desde disco (`#05` + `#06`):**
   * Se cierra la conexión de precarga y se invoca `comandos::iniciar_sgt_en_ruta`, ejecutando `RepositorioSqlite::abrir` (validación física `RD-03` de archivo y WAL, `integrity_check` y `foreign_key_check`) más `Repositorio::cargar` e hidratación de `domain::SistemaTurnos`.
4. **Protocolo de medición (`std::time::Instant`):**
   * Se ejecutan **25 ciclos consecutivos** sobre `Sgt` + `RepositorioSqlite` (elevando el volumen en disco de **200 a 225 turnos** y verificando los **75 snapshots** de evento generados).
   * Todas las operaciones mutadoras pasan por `Sgt::ejecutar`, que aplica la transición sobre un borrador de `SistemaTurnos`, ejecuta `RepositorioSqlite::guardar` en una transacción `BEGIN IMMEDIATE ... COMMIT` en disco y solo tras confirmar actualiza la memoria y emite el snapshot.
   * Al concluir la serie se destruye `Sgt`, se vuelve a abrir la BD con `iniciar_sgt_en_ruta` y se comprueba que los **225 turnos** en disco coinciden exactamente con el estado final en memoria (`RF-11`, `CV-07`, `CV-08`).

---

## 4. Resultados Obtenidos

Mediciones capturadas en perfil `test` (`unoptimized`, escenario más exigente que `release`):

| Operación | Muestras | 1ra muestra (µs) | Mínimo (µs) | Promedio (µs) | P95 (µs) | Máximo (µs) | Máximo (ms) | Umbral `RNF-P01` | Estado |
| :--- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | :---: |
| `cargar_sqlite` (`RF-11`, 200/225 turnos) | 2 | 12 446 | 12 446 | 12 966 | 13 487 | 13 487 | **13.487 ms** | ≤ 2 000 ms | Cumple |
| `obtener_estado` (`RF-03`) | 25 | 59 | 59 | 116 | 198 | 217 | **0.217 ms** | ≤ 2 000 ms | Cumple |
| `registrar_turno` (`RF-02`) | 25 | 898 | 311 | 668 | 1 100 | 1 313 | **1.313 ms** | ≤ 2 000 ms | Cumple |
| `asignar_siguiente` (`RF-04`) | 25 | 924 | 399 | 865 | 1 328 | 1 448 | **1.448 ms** | ≤ 2 000 ms | Cumple |
| `finalizar_atencion` (`RF-06`) | 25 | 618 | 382 | 860 | 1 318 | 1 420 | **1.420 ms** | ≤ 2 000 ms | Cumple |

* **Tiempo máximo en operaciones interactivas (`obtener_estado`, `registrar`, `asignar`, `finalizar`):** `1.448 ms` (**`< 0.08 %`** del presupuesto máximo de `2 000 ms`).
* **Tiempo máximo incluyendo apertura + validación de integridad + carga completa desde disco (`cargar_sqlite`):** `13.487 ms` (**`< 0.68 %`** del presupuesto máximo de `2 000 ms`).

---

## 5. Cómo Reproducir el Benchmark

Desde la raíz del repositorio:

```bash
cargo test --manifest-path src-tauri/Cargo.toml --test benchmark_200_turnos -- --nocapture
```
