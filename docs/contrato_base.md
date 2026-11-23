# Contrato Base del Sistema de Gestión de Turnos (SGT)

> **Propósito:** Definir el contrato único entre el backend en **Rust + SQLite (`rusqlite`) en Tauri** y el frontend en **React + TypeScript** para que los 4 integrantes del equipo trabajen en paralelo (siguiendo el plan descentralizado de sub-issues en [docs/issues_backlog.md](./issues_backlog.md)), cumpliendo con **ADA-09 (`ERS-SGT-001`)** y el estándar de calidad de **ADA-08** en el repositorio `criscnsn/CS-ADA9-gestor-de-turnos`.

---

## 1. Decisiones Técnicas Base

1. **Arquitectura local multi-ventana (sin red):**
   * Una sola instancia de aplicación nativa en Tauri administra un único estado compartido en Rust (`Arc<Mutex<...>>`) y una única base de datos local SQLite (`RC-02`, `RNF-S03`).
   * Se pueden abrir múltiples ventanas (`WebviewWindow`) dentro del mismo proceso (ej. una o varias ventanas **Cliente** y una ventana **Cajero**).
   * En **Vista Cajero**, el operador puede visualizar y alternar entre las 3 ventanillas (`V01`, `V02`, `V03`) desde la misma pantalla.
2. **Manejador de base de datos: SQLite desde Rust (`rusqlite`):**
   * Se utiliza **SQLite** embebido desde el backend Rust (recomendado: crate `rusqlite` con feature `bundled` para evitar dependencias del sistema en Windows, macOS y Linux).
   * **Regla de seguridad (`RNF-S01`, `RNF-M01`):** El frontend **nunca** ejecuta SQL directo (no usar `tauri-plugin-sql` desde TypeScript). Toda consulta o mutación pasa por el dominio Rust y sus comandos IPC.
   * Ubicación de la BD: carpeta de datos de la aplicación resuelta por Tauri (`app.path().app_data_dir()`), ej. `sgt.db` (`RD-01`).
3. **Asignación FIFO sin auto-asignación al llegar (`RF-04`, `RN-01`, `RN-02`, `CV-01`):**
   * Al crear un turno, entra en estado `EN_ESPERA` al final de la cola única FIFO.
   * No se asigna automáticamente a ninguna ventanilla al registrarse; la asignación ocurre cuando una ventanilla libre y habilitada ejecuta `"Asignar siguiente"`.
4. **Extensión acordada fuera del ERS base (`RF-13`):**
   * Las ventanillas `V01`, `V02` y `V03` son fijas (`RN-03`, `RC-03`), pero pueden **habilitarse** o **inhabilitarse** cuando están libres.
   * **Invariante estricto:** Siempre debe permanecer al menos **1 ventanilla habilitada**. Por defecto al inicializar la base de datos por primera vez, las 3 ventanillas inician habilitadas.

---

## 2. Modelo de Datos Compartido (Rust ↔ TypeScript)

Todos los payloads IPC se serializan en JSON usando `camelCase` (`#[serde(rename_all = "camelCase")]` en Rust).

### 2.1 Enumeraciones

| Concepto | Valor serializado (JSON) | Etiqueta visible en UI (`RNF-U03`) |
| :--- | :--- | :--- |
| Estado de turno | `"EN_ESPERA"` | `En espera` |
| Estado de turno | `"EN_ATENCION"` | `En atención` |
| Estado de turno | `"ATENDIDO"` | `Atendido` |
| Estado de turno | `"CANCELADO"` | `Cancelado` |
| ID de ventanilla | `"V01"` \| `"V02"` \| `"V03"` | `V01`, `V02`, `V03` |
| Estado de ventanilla | *(derivado de `habilitada` + `turnoActualId`)* | `Libre`, `Ocupado`, `Inhabilitado` |
| Tipo de vista | `"cliente"` \| `"cajero"` | `Vista Cliente` / `Vista Cajero` |

### 2.2 Interfaces TypeScript (equivalente al contrato `serde` en Rust)

```ts
export type VentanillaId = "V01" | "V02" | "V03";

export type EstadoTurno =
  | "EN_ESPERA"
  | "EN_ATENCION"
  | "ATENDIDO"
  | "CANCELADO";

export type TipoVista = "cliente" | "cajero";

export interface Turno {
  /** Identificador legible único, p. ej. "T-0001" (no reutilizable tras reinicio, CV-08). */
  id: string;
  /** Secuencia numérica monotónica creciente (determina el orden FIFO, RN-02). */
  secuencia: number;
  /** Fecha y hora ISO-8601 de creación, p. ej. "2026-11-18T01:00:00Z". */
  fechaHoraRegistro: string;
  estado: EstadoTurno;
  /** Nulo mientras esté EN_ESPERA o CANCELADO sin atención; obligatorio en EN_ATENCION y ATENDIDO. */
  ventanillaAsignada: VentanillaId | null;
}

export interface Ventanilla {
  id: VentanillaId;
  /** Extensión RF-13: false cuando el cajero inhabilita la caja. */
  habilitada: boolean;
  /** ID del turno actualmente EN_ATENCION en esta ventanilla, o null si está libre. */
  turnoActualId: string | null;
}

export interface EstadoSistema {
  ventanillas: [Ventanilla, Ventanilla, Ventanilla];
  /** Turnos EN_ESPERA ordenados ascendentemente por `secuencia` (RF-03). */
  filaEspera: Turno[];
  /** Todos los turnos registrados en sus 4 estados (RF-10). */
  historial: Turno[];
}

export type CodigoError =
  | "VENTANILLA_INVALIDA"
  | "VENTANILLA_OCUPADA"
  | "VENTANILLA_LIBRE"
  | "VENTANILLA_INHABILITADA"
  | "ULTIMA_VENTANILLA_ACTIVA"
  | "FILA_VACIA"
  | "TURNO_INEXISTENTE"
  | "ESTADO_TURNO_INVALIDO"
  | "ENTRADA_INVALIDA"
  | "ERROR_PERSISTENCIA"
  | "BASE_DATOS_CORRUPTA";

export interface AppError {
  codigo: CodigoError;
  /** Mensaje claro en español explicando la causa y el ID implicado (RNF-U02). */
  mensaje: string;
  /** ID del turno o ventanilla implicado cuando aplique. */
  recursoId: string | null;
}
```

---

## 3. Esquema y Reglas de SQLite (`RD-01`, `RD-02`, `RD-03`)

### 3.1 Tablas SQL

```sql
PRAGMA foreign_keys = ON;
PRAGMA journal_mode = WAL;

CREATE TABLE IF NOT EXISTS secuencias (
    clave TEXT PRIMARY KEY,
    ultimo_valor INTEGER NOT NULL CHECK (ultimo_valor >= 0)
);

INSERT OR IGNORE INTO secuencias (clave, ultimo_valor)
VALUES ('turno', 0);

CREATE TABLE IF NOT EXISTS ventanillas (
    id TEXT PRIMARY KEY CHECK (id IN ('V01', 'V02', 'V03')),
    habilitada INTEGER NOT NULL DEFAULT 1 CHECK (habilitada IN (0, 1)),
    turno_actual_id TEXT NULL UNIQUE
);

INSERT OR IGNORE INTO ventanillas (id, habilitada, turno_actual_id) VALUES
    ('V01', 1, NULL),
    ('V02', 1, NULL),
    ('V03', 1, NULL);

CREATE TABLE IF NOT EXISTS turnos (
    id TEXT PRIMARY KEY,
    secuencia INTEGER NOT NULL UNIQUE CHECK (secuencia > 0),
    fecha_hora_registro TEXT NOT NULL,
    estado TEXT NOT NULL CHECK (estado IN ('EN_ESPERA', 'EN_ATENCION', 'ATENDIDO', 'CANCELADO')),
    ventanilla_asignada TEXT NULL CHECK (
        ventanilla_asignada IS NULL OR ventanilla_asignada IN ('V01', 'V02', 'V03')
    )
);
```

### 3.2 Garantías obligatorias en persistencia

1. **Atomicidad (`RD-02`, `CV-09`, `RNF-R02`):**
   * Toda operación mutadora (`registrar_turno`, `asignar_siguiente`, `finalizar_atencion`, `cancelar_turno`, `cambiar_estado_ventanilla`) debe ejecutarse dentro de una **transacción SQLite** (`BEGIN IMMEDIATE ... COMMIT`).
   * **Regla memoria ↔ disco:** Solo después de que `tx.commit()` devuelva `Ok(())`, el backend actualiza el estado en memoria (si mantiene caché) y emite el evento IPC `"sgt://estado-actualizado"`. Si falla la escritura, se hace `ROLLBACK`, la memoria queda intacta y se retorna `AppError { codigo: "ERROR_PERSISTENCIA", ... }` (`RF-12`).
2. **IDs irrepetibles tras reinicio (`CV-08`):**
   * El contador `secuencias.ultimo_valor` se incrementa dentro de la misma transacción que inserta el nuevo turno. Nunca se calcula con `MAX(secuencia) + 1` ni contando filas.
   * Formato recomendado de ID: `T-0001`, `T-0002`, etc. (padding a 4 dígitos o más si supera 9999).
3. **Detección segura de BD dañada al arrancar (`RD-03`):**
   * Al inicializar, ejecutar validación de integridad (`PRAGMA integrity_check`) y comprobar consistencia básica entre `ventanillas.turno_actual_id` y `turnos.estado = 'EN_ATENCION'`.
   * Si la BD es ilegible o está dañada, **prohibido borrarla o sobrescribirla automáticamente**: devolver `BASE_DATOS_CORRUPTA` para que la UI informe al usuario de forma explícita.

---

## 4. Contrato IPC: Comandos y Eventos Tauri

### 4.1 Comandos (`invoke`)

| Comando Rust (`#[tauri::command]`) | Parámetros (TS) | Retorno (`Result<T, AppError>`) | Reglas y validaciones clave |
| :--- | :--- | :--- | :--- |
| `obtener_estado` | *(ninguno)* | `EstadoSistema` | Devuelve ventanillas, fila ordenada por `secuencia` y el historial completo (`RF-01`, `RF-03`, `RF-10`). |
| `registrar_turno` | *(ninguno)* | `Turno` | Incrementa secuencia, crea turno `EN_ESPERA`, persiste en SQLite, emite evento (`RF-02`, `RN-01`, `RN-02`). |
| `asignar_siguiente` | `{ ventanillaId: VentanillaId }` | `Turno` | Valida ventanilla existente, habilitada y libre (`RF-04`, `RF-05`, `RN-04`, `RN-05`, `RN-10`); toma menor `secuencia` en `EN_ESPERA`; pasa a `EN_ATENCION`. |
| `finalizar_atencion` | `{ ventanillaId: VentanillaId }` | `Turno` | Valida que la ventanilla tenga turno `EN_ATENCION` (`RF-06`, `RF-07`, `RN-08`); pasa turno a `ATENDIDO` y libera ventanilla. |
| `cancelar_turno` | `{ turnoId: string }` | `Turno` | Valida formato no vacío (`RNF-S04`), existencia y estado `EN_ESPERA` (`RF-08`, `RF-09`, `RN-07`). Cambia a `CANCELADO` sin borrar de BD. |
| `cambiar_estado_ventanilla` | `{ ventanillaId: VentanillaId, habilitada: boolean }` | `Ventanilla` | Extensión `RF-13`: si `habilitada === false`, rechaza si la ventanilla está ocupada (`VENTANILLA_OCUPADA`) o si es la única habilitada (`ULTIMA_VENTANILLA_ACTIVA`). |
| `abrir_ventana` | `{ vista: TipoVista }` | `void` | Crea o enfoca una `WebviewWindow` (`WebviewWindowBuilder`) usando query params `?vista=cliente` o `?vista=cajero` (permite múltiples ventanas Cliente y enfoca o crea la ventana Cajero). |

### 4.2 Evento en tiempo real (`listen`)

* **Nombre del evento:** `"sgt://estado-actualizado"`
* **Emisor:** Backend Rust inmediatamente después de confirmar cualquier transacción mutadora exitosa.
* **Payload:** `EstadoSistema` completo.
* **Comportamiento del frontend:** Toda ventana (`Cliente` o `Cajero`) llama a `obtener_estado()` al montarse y se suscribe a `"sgt://estado-actualizado"` para re-renderizar fila, ventanillas e historial de forma consistente (`RI-02`).

---

## 5. Reglas de Validación y Contrato UI

1. **Validación en dominio (`RNF-S01`, `RNF-S04`):**
   * Todas las restricciones (`RN-01`–`RN-10` y `RF-13`) se validan en el dominio Rust aunque el frontend deshabilite botones.
   * `turnoId` y `ventanillaId` se sanitizan (`trim`) y validan antes de consultar la BD.
2. **Etiquetas y confirmaciones (`RI-01`, `RI-03`, `RNF-U04`):**
   * Vista Cajero debe mostrar botones diferenciados con texto exacto: **`"Asignar siguiente"`**, **`"Finalizar"`**, **`"Cancelar"`** (con entrada por ID) y **`"Inhabilitar"` / `"Habilitar"`**, más consulta de **`"Historial"`**.
   * Vista Cliente debe mostrar: botón **`"Sacar turno"`**, matriz de fila de espera, estado de `V01`–`V03` (`Libre`, `Ocupado`, `Inhabilitado`), cuántos turnos hay delante y botón **`"Cancelar"`** con **modal de confirmación explícita** antes de invocar `cancelar_turno`.
3. **Mocking temporal para desarrollo paralelo en frontend (`#02-B`):**
   * Definir las interfaces espejo en `src/types/` (o `src/shared/`) y un adaptador mock en memoria que implemente exactamente las firmas de la sección 4.1. Mientras el backend (`#03`–`#06`) está en construcción, el frontend puede desarrollarse y probarse en navegador/`Vitest` contra este mock.
   * En Rust (`#02-A`), los modelos equivalentes con `serde` (`camelCase`) viven en `src-tauri/src/models/`.
