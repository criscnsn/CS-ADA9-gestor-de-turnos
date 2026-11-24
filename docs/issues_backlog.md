# Backlog de Issues para GitHub Projects (ADA-08 + ADA-09)

> **Proyecto:** ADA-09 Sistema de Gestión de Turnos (SGT) · **Repositorio:** `criscnsn/CS-ADA9-gestor-de-turnos`
>
> Este archivo concentra el plan actualizado y **descentralizado en sub-issues** para **Semana 1 (Bloque 0 y Bloque 1)** y **Bloque 2 (Persistencia SQLite, IPC y Multi-Ventana)** según `Issues_Semana1_y_Bloque2_ADA9.pdf`, junto con los **Bloques 3 y 4** para completar las 3 semanas del proyecto cumpliendo **`ERS-SGT-001` (ADA-09)** y **`CDS_ADA08_Equipo7.pdf` (ADA-08)**.

---

## Tabla Resumen de Sub-issues (Semana 1 y Bloque 2)

| Issue Original | Sub-issue | Rama Git | Enfoque / Responsable sugerido | Estado GitHub |
| :--- | :--- | :--- | :--- | :--- |
| **#01** Setup y CI | **#01-A** Limpieza Rust y `rust-toolchain.toml` | `feature/1a-setup-rust-base` | Backend (Cristopher / Gael) | Publicada en **GH #3** (OPEN) |
| **#01** Setup y CI | **#01-B** Tooling Frontend y `.tsx` vacíos | `feature/1b-setup-frontend-tooling` | Frontend (Diego / Daniel) | Publicada en **GH #3** (OPEN) |
| **#01** Setup y CI | **#01-C** GitHub Actions CI y PR Template | `feature/1c-setup-github-ci` | DevOps / Equipo (Diego / Daniel) | Publicada en **GH #3** (OPEN) |
| **#02** Contrato Base | **#02-A** Modelos Rust (`serde`) | `feature/2a-contrato-tipos-rust` | Backend (Cristopher / Gael) | Pendiente de crear |
| **#02** Contrato Base | **#02-B** Tipos TS y Cliente Mock | `feature/2b-contrato-mock-ts` | Frontend / Fullstack (Diego / Daniel / Gael) | Pendiente de crear |
| **#03** Dominio FIFO | **#03-A** Registro FIFO, secuencia y consultas | `feature/3a-dominio-registro-consultas` | Backend (Cristopher) | Pendiente de crear |
| **#03** Dominio FIFO | **#03-B** Máquina de estados y validaciones | `feature/3b-dominio-transiciones-turnos` | Backend (Cristopher) | Pendiente de crear |
| **#04** Extensión RF-13 y Tests | **#04** Regla `RF-13` y suite `CV-01`–`CV-06`, `CV-10` | `feature/4-dominio-rf13-tests-cv` | Backend (Cristopher) | Pendiente de crear |
| **#05** Persistencia SQLite | **#05-A** Esquema SQLite (`rusqlite`) y transacciones | `18-definir-el-esquema-de-la-base-de-datos-en-sqlite` / `feature/5a-sqlite-esquema-transacciones` | Backend (Gael) | Publicada en **GH #18** / Relacionada con **GH #2** |
| **#05** Persistencia SQLite | **#05-B** Corrupción (`RD-03`) y tests `CV-07`–`CV-09` | `feature/5b-sqlite-corrupcion-tests-cv` | Backend (Gael) | Relacionada con **GH #2** |
| **#06** Comandos IPC Tauri | **#06** Comandos IPC, `Arc<Mutex<...>>` y eventos | `feature/6-ipc-commands-eventos` | Backend (Gael) | Pendiente de crear |
| **#07** Multi-Ventana y Perf | **#07-A** Gestor multi-ventana (`WebviewWindow`) | `feature/7a-multiventana-webview` | Backend (Gael) | Pendiente de crear |
| **#07** Multi-Ventana y Perf | **#07-B** Benchmark de rendimiento (200 turnos) | `feature/7b-benchmark-200-turnos` | Backend / QA (Gael / Equipo) | Pendiente de crear |

---

## Mapa de Dependencias (Orden de Ejecución en 3 Semanas)

```text
Semana 1 (Días 1-3 · Bloque 0):
  #01-A (Rust base)  ┐
  #01-B (FE tooling) ├──► #02-A (Modelos Rust serde) ──┐
  #01-C (CI + PR)    ┘    #02-B (Tipos TS + Mock)    ──┤
                                                       │
        ┌──────────────────────────────────────────────┴─────────────────────────┐
        ▼                                                                        ▼
Semana 1-2 (Bloque 1 y Bloque 2 · Backend Rust + SQLite):            Semana 1-2 (Bloque 3 · Frontend paralelo):
  #03-A (Registro/FIFO) ─► #03-B (Transiciones) ─► #04 (RF-13+CV)      #08 (sgtClient/Mock + Componentes base)
                                                        │                       │
  #05-A (SQLite esquema/tx) ─► #05-B (RD-03 + CV-07..09)┤                       ├──► #09 (Vista Cajero + Historial)
                                                        ▼                       └──► #10 (Vista Cliente + Matriz)
  #06 (Comandos IPC + Mutex + Eventos) ─► #07-A (Multi-ventana)                          │
                                       └─► #07-B (Benchmark 200 turnos)                  ▼
                                                        └──────────────► #11 (Integración Tauri Real + Vitest)
                                                                                         │
                                                                                         ▼
Semana 3 (Bloque 4 · Cierre): #12 (E2E WebdriverIO) ─► #13 (Auditoría Seguridad) ─► #14 (Docs, Trazabilidad y Revisión Final)
```

> **Clave anti-bloqueos:** Al dividir `#01` en `#01-A/B/C` y `#02` en `#02-A/B`, los 4 integrantes evitan pisarse archivos desde el día 1. En cuanto entren `#02-A` y `#02-B`, **Cristopher + Gael** avanzan en `#03-A` a `#07-B` (Rust + SQLite) mientras **Daniel + Diego** desarrollan `#08`–`#10` (UI + Vitest usando el mock del contrato), integrando en `#11`.

---

## Parte 1: Issues de la Semana 1 (Bloque 0 y Bloque 1)

### Bloque 0: Entorno, CI y Contrato Base (Días 1–3)

#### Issue #01 — Configurar entorno estándar ADA-08, limpieza inicial y CI en GitHub Actions
* **Estado GitHub:** Publicada en **GitHub Issue #3 (OPEN)**
* **Rama original:** `feature/1-setup-entorno-ci` · **Asignados actuales:** Diego, Cristopher, Daniel
* **Trazabilidad:** ADA-08 (Clippy, rustfmt, ESLint, Prettier, Vitest, GitHub Actions, Indicadores) · `RNF-M02`, `RNF-M04`
* **Objetivo:** Dejar `main` compilando sin errores ni advertencias, con herramientas de calidad configuradas y pipeline automático en GitHub Actions.

**División en 3 sub-issues (para evitar conflictos entre los 3 asignados):**

##### Sub-issue #01-A — Limpieza inicial de módulos Rust y `rust-toolchain.toml`
* **Rama:** `feature/1a-setup-rust-base` · **Responsable sugerido:** Backend (Cristopher / Gael)
* **Tareas concretas:**
  1. Reubicar los módulos Rust vacíos (`state.rs`, `models/`, `commands/`, `services/`) dentro de `src-tauri/src/` y declararlos en `src-tauri/src/lib.rs`.
  2. Fijar `src-tauri/rust-toolchain.toml` compatible con `edition = "2024"` *(archivo base ya incluido en el commit inicial de entorno)*.
  3. Verificar que `cargo check`, `cargo fmt --check` y `cargo clippy` pasen limpios en local.
* **Criterios de aceptación:**
  * [ ] No quedan módulos Rust sueltos fuera de `src-tauri/src/`.
  * [ ] `cargo check`, `cargo fmt --check` y `cargo clippy --all-targets -- -D warnings` pasan en verde.

##### Sub-issue #01-B — Configuración de Linters/Tests Frontend y limpieza de `.tsx` vacíos
* **Rama:** `feature/1b-setup-frontend-tooling` · **Responsable sugerido:** Frontend (Diego / Daniel)
* **Tareas concretas:**
  1. Agregar componentes base mínimos o `export {};` en los 4 archivos `.tsx` vacíos (`MainLayout`, `VentanillaLayout`, `TurnosPage`, `VentanillaPage`) para eliminar errores de `isolatedModules: true`.
  2. Instalar y configurar en `package.json`: `eslint`, `prettier`, `vitest`, `@testing-library/react`, `@testing-library/jest-dom` *(dependencias y archivos de configuración base ya agregados en `main`; validar y ajustar si hace falta)*.
  3. Agregar y verificar scripts: `pnpm lint`, `pnpm format:check`, `pnpm test` y `pnpm build`.
* **Criterios de aceptación:**
  * [ ] Los 4 archivos `.tsx` vacíos tienen módulo válido (`export {};` o componente mínimo).
  * [ ] `pnpm install && pnpm lint && pnpm format:check && pnpm test && pnpm build` pasan en verde.

##### Sub-issue #01-C — Pipeline CI en GitHub Actions y Plantilla de PR
* **Rama:** `feature/1c-setup-github-ci` · **Responsable sugerido:** DevOps / Equipo (Diego / Daniel)
* **Tareas concretas:**
  1. Crear `.github/PULL_REQUEST_TEMPLATE.md` con checkboxes de pruebas (`cargo test`, `vitest`), trazabilidad ERS y registro obligatorio de uso de IA (ADA-08).
  2. Crear `.github/workflows/ci.yml` ejecutando en cada PR: `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test`, `pnpm lint`, `pnpm format:check`, `pnpm test` y `pnpm build`.
* **Criterios de aceptación:**
  * [ ] El workflow de GitHub Actions corre en verde al abrir un PR hacia `main` (Windows, macOS y Linux).
  * [ ] Se mide y anota el tiempo base de preparación del entorno (Indicador 1 de ADA-08).

---

#### Issue #02 — Implementar tipos compartidos del Contrato Base (Rust ↔ TS) y cliente mock
* **Estado GitHub:** Pendiente de crear en GitHub
* **Rama original:** `feature/2-contrato-ipc-tipos` · **Responsables sugeridos:** Gael + Cristopher
* **Trazabilidad:** `RNF-M01`, `RI-02`, `RF-01`–`RF-12`, `RF-13`
* **Objetivo:** Traducir [docs/contrato_base.md](./contrato_base.md) a código fuente para desbloquear el desarrollo paralelo del frontend desde el día 3.

**División en 2 sub-issues (Backend vs Frontend):**

##### Sub-issue #02-A — Modelos y errores del Contrato Base en Rust (`serde`)
* **Rama:** `feature/2a-contrato-tipos-rust` · **Responsable sugerido:** Backend (Cristopher / Gael)
* **Tareas concretas:**
  1. Definir en `src-tauri/src/models/` los structs y enums con `serde` (serialización `camelCase`): `Turno`, `Ventanilla`, `EstadoTurno`, `VentanillaId`, `EstadoSistema`, `AppError`, `CodigoError`.
  2. Escribir prueba unitaria de serialización/deserialización JSON en `cargo test` garantizando coincidencia 100% con el contrato.
* **Criterios de aceptación:**
  * [ ] Todos los structs y enums serializan en `camelCase` exactamente como indica `docs/contrato_base.md`.
  * [ ] `cargo test` valida serialización y deserialización sin advertencias de `clippy`.

##### Sub-issue #02-B — Interfaces TypeScript del Contrato Base y Cliente Mock en memoria
* **Rama:** `feature/2b-contrato-mock-ts` · **Responsable sugerido:** Frontend / Fullstack (Diego / Daniel / Gael)
* **Tareas concretas:**
  1. Definir en `src/types/` (o `src/shared/`) las interfaces espejo exactas de TypeScript y las firmas del cliente IPC.
  2. Implementar un adaptador mock en memoria para el frontend que soporte todas las operaciones del contrato y validarlo con pruebas unitarias en `Vitest`.
* **Criterios de aceptación:**
  * [ ] Interfaces TypeScript coinciden 100% con los payloads JSON de Rust.
  * [ ] El adaptador mock cubre `obtener_estado`, `registrar_turno`, `asignar_siguiente`, `finalizar_atencion`, `cancelar_turno` y `cambiar_estado_ventanilla`, verificado en `Vitest`.

---

### Bloque 1: Dominio Puro en Rust (Semana 1)

#### Issue #03 — Implementar núcleo FIFO, máquina de estados de Turnos y validaciones de negocio
* **Estado GitHub:** Pendiente de crear en GitHub
* **Rama original:** `feature/3-dominio-fifo-turnos` · **Responsable sugerido:** Cristopher Israel Cen Santana
* **Trazabilidad:** `RF-01`–`RF-10`, `RF-12`, `RN-01`–`RN-10`, `RNF-S01`, `RNF-S04`, `RNF-M01`
* **Objetivo:** Implementar la lógica pura de dominio del SGT en Rust sin dependencias de Tauri ni de SQLite (`RNF-M01`).

**División en 2 sub-issues:**

##### Sub-issue #03-A — Registro de turnos con secuencia monotónica, fila FIFO e historial
* **Rama:** `feature/3a-dominio-registro-consultas` · **Responsable sugerido:** Backend (Cristopher)
* **Tareas concretas:**
  1. Implementar creación de turnos con secuencia monotónica creciente (`RN-02`), ID único formateado (`T-0001`), fecha/hora de registro (`chrono` o `time`) y estado inicial `EN_ESPERA` (`RF-01`, `RF-02`).
  2. Implementar consulta de fila ordenada ascendentemente por `secuencia` (`RF-03`) y consulta de historial completo (`RF-10`).
* **Criterios de aceptación:**
  * [ ] El módulo de dominio no importa `tauri` ni `rusqlite` (`RNF-M01`).
  * [ ] El orden de la fila depende estrictamente de `secuencia` (`RN-02`).

##### Sub-issue #03-B — Máquina de estados de atención (asignar, finalizar, cancelar) y errores
* **Rama:** `feature/3b-dominio-transiciones-turnos` · **Responsable sugerido:** Backend (Cristopher)
* **Tareas concretas:**
  1. Implementar `asignar_siguiente(ventanilla_id)`: seleccionar el turno `EN_ESPERA` con menor secuencia, validando ventanilla libre y fila no vacía (`RF-04`, `RF-05`, `RN-04`, `RN-05`, `RN-10`).
  2. Implementar `finalizar_atencion(ventanilla_id)`: cambiar turno a `ATENDIDO` y liberar ventanilla (`RF-06`, `RF-07`, `RN-08`).
  3. Implementar `cancelar_turno(turno_id)`: validar entrada (`RNF-S04`), rechazar si no existe o no está `EN_ESPERA` (`RF-08`, `RF-09`, `RN-07`), pasar a `CANCELADO` conservándolo en historial (`RN-09`).
  4. Devolver mensajes descriptivos en `AppError` incluyendo el ID implicado (`RNF-U02`, `RNF-R02`) y verificar `cargo doc --no-deps` sin advertencias.
* **Criterios de aceptación:**
  * [ ] Ninguna operación inválida altera el estado previo (`RNF-R02`).
  * [ ] `cargo doc --no-deps` genera documentación sin advertencias en los structs/métodos públicos.

---

#### Issue #04 — Implementar extensión `RF-13` (habilitar/inhabilitar ventanilla) y tests `CV-01`..`CV-06`, `CV-10`
* **Estado GitHub:** Pendiente de crear en GitHub
* **Rama:** `feature/4-dominio-rf13-tests-cv` · **Responsable sugerido:** Cristopher Israel Cen Santana
* **Trazabilidad:** `RF-13`, `CV-01`–`CV-06`, `CV-10`, `RNF-M03`, `RNF-R02`
* **Objetivo:** Agregar la regla de habilitar/inhabilitar cajas y cubrir el dominio puro con pruebas unitarias automatizadas en `cargo test`.
* **Tareas concretas:**
  1. Regla `cambiar_estado_ventanilla(ventanilla_id, habilitada)`:
     * Rechazar inhabilitación si la ventanilla tiene un turno `EN_ATENCION`.
     * Rechazar inhabilitación si dejaría **0 ventanillas activas** (código `ULTIMA_VENTANILLA_ACTIVA`).
     * En `asignar_siguiente`, rechazar con `VENTANILLA_INHABILITADA` si la caja está desactivada.
  2. Suite de pruebas unitarias en `cargo test`:
     * `CV-01`: Registrar T1, T2, T3; asignar en V02 y luego en V01 → T1 en V02, T2 en V01, T3 en espera.
     * `CV-02`: Intentar asignar en V02 ocupada → rechazo sin cambios de estado.
     * `CV-03`: Con las 3 ventanillas ocupadas → no permite cuarta atención simultánea.
     * `CV-04`: Finalizar V02 y volver a asignar → libera caja y toma el siguiente turno FIFO.
     * `CV-05`: Cancelar T2 en espera → pasa a `CANCELADO` y la siguiente asignación lo omite.
     * `CV-06`: Cancelar turno `EN_ATENCION`, `ATENDIDO`, `CANCELADO` o ID vacío/inexistente → rechazo sin cambios.
     * `CV-10`: El historial conserva todos los turnos `ATENDIDO` y `CANCELADO`.
     * Casos borde `RF-13`: inhabilitar 2 cajas permitido, intentar inhabilitar la 3ra bloqueado, inhabilitar caja ocupada bloqueado.
* **Criterios de aceptación:**
  * [ ] Todas las pruebas (`CV-01`–`CV-06`, `CV-10` y `RF-13`) pasan en `cargo test`.
  * [ ] `cargo clippy` y `cargo fmt --check` pasan sin warnings.

---

## Parte 2: Issues del Bloque 2 (Persistencia SQLite, IPC y Multi-Ventana · Semanas 1–2)

### Issue #05 — Implementar capa de persistencia en SQLite con transacciones atómicas y detección de corrupción
* **Estado GitHub:** Relacionada con **GH Issue #2**
* **Rama original:** `feature/5-persistencia-sqlite` · **Responsable sugerido:** Gael Antonio Ayuso Contreras
* **Trazabilidad:** `RF-11`, `RF-12`, `RD-01`, `RD-02`, `RD-03`, `CV-07`, `CV-08`, `CV-09`, `RNF-R01`
* **Objetivo:** Persistir turnos, ventanillas (incluido `habilitada`) y secuencia monotónica en **SQLite** local con integridad transaccional.

**División en 2 sub-issues:**

#### Sub-issue #05-A — Esquema SQLite (`rusqlite`) y repositorio transaccional
* **Rama:** `18-definir-el-esquema-de-la-base-de-datos-en-sqlite` (esquema inicial, **GH #18**) / `feature/5a-sqlite-esquema-transacciones` (repositorio transaccional) · **Responsable sugerido:** Backend (Gael)
* **Tareas concretas:**
  1. Agregar `rusqlite` (con feature `bundled`) a `src-tauri/Cargo.toml` y verificar con `cargo audit`.
  2. Crear migración/inicializador de esquema SQLite según [docs/esquema_bd.md](./esquema_bd.md) y [docs/contrato_base.md](./contrato_base.md) (tablas `schema_migrations`, `secuencias`, `ventanillas`, `turnos`, `PRAGMA journal_mode=WAL`, `PRAGMA foreign_keys=ON`, índices y triggers).
  3. Implementar operaciones transaccionales atómicas (`BEGIN IMMEDIATE ... COMMIT` / `ROLLBACK` automático ante fallo).
* **Criterios de aceptación:**
  * [ ] El esquema SQLite se crea automáticamente en una BD nueva e inicializa `V01`, `V02`, `V03` habilitadas y `secuencias('turno') = 0`.
  * [ ] Todas las mutaciones corren en transacciones atómicas y el frontend nunca ejecuta SQL directo.

#### Sub-issue #05-B — Detección de corrupción (`RD-03`) y pruebas de resiliencia (`CV-07` a `CV-09`)
* **Rama:** `feature/5b-sqlite-corrupcion-tests-cv` · **Responsable sugerido:** Backend (Gael)
* **Tareas concretas:**
  1. Al arrancar, validar integridad (`PRAGMA integrity_check` y consistencia `ventanillas` ↔ `turnos` `EN_ATENCION`); si el archivo está dañado, devolver `BASE_DATOS_CORRUPTA` **sin sobrescribir la BD** (`RD-03`).
  2. Implementar mecanismo inyectable de fallo controlado de escritura para probar `CV-09`.
  3. Escribir pruebas automatizadas en `cargo test` (usando BD temporal en disco) para `CV-07` (recuperación tras reinicio con turnos activos), `CV-08` (no reutilizar IDs tras reinicio), `CV-09` (interrupción simulada sin cambios parciales) y `RD-03` (archivo corrupto).
* **Criterios de aceptación:**
  * [ ] `CV-07`, `CV-08`, `CV-09` y `RD-03` verificados en `cargo test`.
  * [ ] Ningún dato ni secuencia se pierde o reutiliza al cerrar y reabrir la conexión SQLite.

---

### Issue #06 — Exponer comandos IPC en Tauri, estado `Arc<Mutex<...>>` y evento en tiempo real
* **Estado GitHub:** Pendiente de crear en GitHub
* **Rama:** `feature/6-ipc-commands-eventos` · **Responsable sugerido:** Gael Antonio Ayuso Contreras
* **Trazabilidad:** `RF-01`–`RF-13`, `RI-02`, `RD-02`, `RNF-S01`, `RNF-R02`
* **Objetivo:** Conectar el dominio + SQLite con el frontend a través de comandos `#[tauri::command]` seguros y sincronización reactiva.
* **Tareas concretas:**
  1. Configurar estado gestionado (`tauri::State` con `Arc<Mutex<...>>`) inicializando la BD SQLite en `app_data_dir()` dentro de `src-tauri/src/lib.rs`.
  2. Implementar todos los comandos del contrato: `obtener_estado`, `registrar_turno`, `asignar_siguiente`, `finalizar_atencion`, `cancelar_turno`, `cambiar_estado_ventanilla`.
  3. Garantizar el patrón estricto: **SQLite `commit()` exitoso → actualizar estado en memoria → emitir evento `"sgt://estado-actualizado"`** (`app.emit(...)`). Si SQLite falla, no modificar memoria ni emitir éxito (`RF-12`, `RD-02`).
  4. Registrar todos los comandos en `tauri::generate_handler![]` y verificar que ante fallo de BD la memoria conserve el estado previo exacto.
* **Criterios de aceptación:**
  * [ ] Todos los comandos responden con `EstadoSistema` / `Turno` / `Ventanilla` o `AppError` tipado.
  * [ ] Ante fallo en BD, el estado en memoria coincide exactamente con el estado previo.

---

### Issue #07 — Implementar gestor multi-ventana (`WebviewWindow`) y prueba de rendimiento con 200 turnos
* **Estado GitHub:** Pendiente de crear en GitHub
* **Rama original:** `feature/7-multiventana-rendimiento` · **Responsable sugerido:** Gael Antonio Ayuso Contreras
* **Trazabilidad:** `RNF-P01`, `RI-02`, `RC-02`
* **Objetivo:** Permitir abrir simultáneamente ventanas Cliente y ventana Cajero dentro de la misma instancia local y validar el umbral de rendimiento de 2 segundos.

**División en 2 sub-issues:**

#### Sub-issue #07-A — Comando `abrir_ventana` y sincronización multi-ventana (`WebviewWindowBuilder`)
* **Rama:** `feature/7a-multiventana-webview` · **Responsable sugerido:** Backend (Gael)
* **Tareas concretas:**
  1. Implementar comando `abrir_ventana(vista: "cliente" | "cajero")` usando `tauri::WebviewWindowBuilder` apuntando a query params (`?vista=cliente`, `?vista=cajero`), permitiendo múltiples ventanas Cliente y enfocando o creando la ventana Cajero.
  2. Verificar que toda ventana nueva se hidrate con `obtener_estado` y reciba en vivo `"sgt://estado-actualizado"` sin condiciones de carrera.
* **Criterios de aceptación:**
  * [ ] Dos o más ventanas abiertas en la misma máquina se sincronizan en tiempo real sin condiciones de carrera.

#### Sub-issue #07-B — Benchmark de rendimiento con 200 turnos en SQLite (`RNF-P01`)
* **Rama:** `feature/7b-benchmark-200-turnos` · **Responsable sugerido:** Backend / QA (Gael / Equipo)
* **Tareas concretas:**
  1. Implementar prueba automatizada/benchmark con **200 turnos almacenados en SQLite** verificando que registro, consulta de fila, asignación y finalización responden en **≤ 2 segundos** (`RNF-P01`).
  2. Documentar las mediciones obtenidas y las especificaciones de hardware del equipo de prueba.
* **Criterios de aceptación:**
  * [ ] Medición de `RNF-P01` con 200 turnos documentada y < 2s por operación.

---

## Parte 3: Bloque 3 — Interfaces de Usuario y Pruebas Frontend (Semana 2)

### Issue #08 — Construir capa compartida frontend (`sgtClient`), navegación de vistas y componentes base accesibles
* **Responsable sugerido:** Daniel Alejandro Cauich Uicab + Diego Gervacio de la Paz
* **Rama:** `feature/8-frontend-base-shared`
* **Trazabilidad:** `RI-01`–`RI-03`, `RNF-U02`, `RNF-U03`, `RNF-U04`
* **Objetivo:** Crear la infraestructura común de React para ambas vistas (cliente IPC real/mock, manejo de errores, modal de confirmación y pantalla selector/launcher).
* **Tareas concretas:**
  1. Implementar hook/store reactivo que cargue `obtener_estado()` y escuche `"sgt://estado-actualizado"` (con fallback al mock de `#02-B` para correr en `Vitest`/browser).
  2. Crear componentes reutilizables:
     * **Badge de estado** que combine color + texto explícito (`En espera`, `En atención`, `Atendido`, `Cancelado`, `Libre`, `Ocupado`, `Inhabilitado` — `RNF-U03`).
     * **Modal de confirmación explícita** reutilizable para cancelaciones (`RNF-U04`).
     * **Banner/Toast de retroalimentación** que muestre confirmaciones y errores con causa e ID (`RF-12`, `RNF-U02`).
     * **Pantalla o barra inicial** que permita elegir Vista Cliente / Vista Cajero o abrir otra ventana (`abrir_ventana` vía `?vista=cliente` / `?vista=cajero`).
     * Manejo visible de error crítico `BASE_DATOS_CORRUPTA` (`RD-03`).
* **Criterios de aceptación:**
  * [ ] Pruebas en `Vitest` verifican que rechazar el modal de confirmación no dispara `cancelar_turno` (`RNF-U04`).
  * [ ] `ESLint` y `Prettier` pasan sin advertencias.

---

### Issue #09 — Implementar Vista Cajero (estilo *Papers, Please*, cambio entre `V01`–`V03` e Historial)
* **Responsable sugerido:** Daniel Alejandro Cauich Uicab
* **Rama:** `feature/9-vista-cajero`
* **Trazabilidad:** `RF-01`, `RF-03`–`RF-10`, `RF-12`, `RF-13`, `RI-01`–`RI-03`, `RNF-U01`–`RNF-U04`
* **Objetivo:** Construir la interfaz operativa del cajero con estética inspirada en *Papers, Please* donde pueda ver la fila FIFO, moverse entre las 3 ventanillas (`V01`, `V02`, `V03`), atender y consultar el historial.
* **Tareas concretas:**
  1. **Selector/zona de ventanillas (`V01`, `V02`, `V03`):** Mostrar estado de las 3 cajas y permitir alternar fácilmente qué ventanilla se está operando en la zona exclusiva de atención (o visualizar las 3 tarjetas con controles por caja).
  2. **Controles explícitos por ventanilla (`RI-03`, `RF-13`):**
     * Botón **`"Asignar siguiente"`**
     * Botón **`"Finalizar"`**
     * Botón **`"Inhabilitar"` / `"Habilitar"`** (reflejando la regla de al menos 1 activa).
  3. **Panel de fila FIFO en tiempo real:** Mostrar turnos `EN_ESPERA` en orden ascendente de secuencia con ID y fecha/hora (`RF-03`).
  4. **Cancelación por ID (`RI-01`, `RF-08`, `RNF-U04`):** Campo/selector por ID de turno + botón **`"Cancelar"`** que abre modal de confirmación.
  5. **Pestaña / panel `"Historial"` (`RF-10`, `CV-10`):** Tabla con todos los turnos en los 4 estados mostrando ID, fecha/hora, estado (texto + indicador visual) y ventanilla asignada.
* **Criterios de aceptación:**
  * [ ] Todas las operaciones `RF-01`–`RF-13` son ejecutables desde la interfaz sin herramientas externas (`RNF-U01`).
  * [ ] Pruebas en `Vitest` cubren renderizado de las 3 ventanillas, botones requeridos, flujo de confirmación y tabla de historial.

---

### Issue #10 — Implementar Vista Cliente (Matriz de sala de espera, estado de ventanillas y flujo de turno)
* **Responsable sugerido:** Diego Gervacio de la Paz
* **Rama:** `feature/10-vista-cliente`
* **Trazabilidad:** `RF-01`–`RF-03`, `RF-08`, `RF-12`, `RI-01`–`RI-03`, `RNF-U02`–`RNF-U04`
* **Objetivo:** Construir la interfaz simplificada del cliente con matriz visual de sala de espera, tablero de las 3 ventanillas y flujo de sacar/seguir/cancelar turno.
* **Tareas concretas:**
  1. **Tablero de ventanillas (`V01`, `V02`, `V03`):** Indicar con texto + estilo visual cuáles están `Libre`, `Ocupado` (con turno actual) o `Inhabilitado` (`RNF-U03`).
  2. **Matriz visual de sala de espera:** Representar gráficamente los turnos `EN_ESPERA` en orden FIFO resaltando el/los turno(s) del usuario en esa ventana e indicando explícitamente **cuántos turnos hay delante**.
  3. **Flujo principal:**
     * Botón **`"Sacar turno"`** visible inicialmente; al presionarlo registra el turno y muestra su ID, posición en fila y aviso cuando pase a `EN_ATENCION` (indicando la ventanilla).
     * Permitir también sacar otro turno (para probar múltiples clientes en una sola máquina) sin perder seguimiento, y regresar a la pantalla principal de `"Sacar turno"` cuando el turno actual pasa a `ATENDIDO` o `CANCELADO`.
     * Botón **`"Cancelar"`** disponible mientras el turno esté `EN_ESPERA`, con modal de confirmación explícita (`RNF-U04`).
* **Criterios de aceptación:**
  * [ ] El usuario ve en todo momento cuántos turnos hay delante suyo y en qué ventanilla es llamado.
  * [ ] La matriz no depende solo de colores para distinguir estados (`RNF-U03`).
  * [ ] Pruebas en `Vitest` cubren sacar turno, cálculo de turnos delante, estado en atención, retorno al finalizar y cancelación confirmada/rechazada.

---

### Issue #11 — Integrar Frontend con Backend Tauri real y validar sincronización multi-ventana
* **Responsable sugerido:** Daniel Alejandro Cauich Uicab + Gael Antonio Ayuso Contreras
* **Rama:** `feature/11-integracion-tauri-ipc`
* **Trazabilidad:** `RI-02`, `RF-01`–`RF-13`, `CV-01`–`CV-10`
* **Objetivo:** Conectar `sgtClient` con `@tauri-apps/api/core` (`invoke`) y `@tauri-apps/api/event` (`listen`) sustituyendo el mock en runtime Tauri.
* **Tareas concretas:**
  1. Conectar todos los comandos IPC y listener `"sgt://estado-actualizado"`.
  2. Probar manualmente en `pnpm tauri dev` abrir 1 Vista Cajero + 2 Vistas Cliente simultáneas y ejecutar el recorrido completo de `CV-01` a `CV-10` más `RF-13`.
  3. Verificar que al cerrar y volver a abrir la app (`pnpm tauri dev`) los datos de SQLite se recuperen intactos (`RF-11`, `CV-07`, `CV-08`).
* **Criterios de aceptación:**
  * [ ] Cero inconsistencias visuales entre ventanas Cliente y Cajero tras registrar, asignar, finalizar, cancelar o inhabilitar.
  * [ ] Estado completo sobrevive al cierre y reinicio de la app en SQLite.

---

## Parte 4: Bloque 4 — Calidad Final, Seguridad y Documentación (Semana 3)

### Issue #12 — Configurar y ejecutar pruebas E2E críticas con `WebdriverIO`
* **Responsable sugerido:** Diego Gervacio de la Paz
* **Rama:** `feature/12-pruebas-e2e-wdio`
* **Trazabilidad:** ADA-08 (`WebdriverIO` para rutas críticas una vez configurado Tauri) · `RNF-U01`
* **Objetivo:** Automatizar el recorrido crítico de extremo a extremo (registrar turno → asignar → finalizar / cancelar con confirmación).
* **Nota técnica importante:** `tauri-driver` soporta **Linux y Windows** (no macOS). Configurar el runner E2E para ejecutarse contra la app Tauri en **GitHub Actions (`ubuntu-latest` con `xvfb-run` + `webkit2gtk-driver` o `windows-latest`)** y/o contra el frontend servido por Vite en local si el desarrollador usa macOS.
* **Criterios de aceptación:**
  * [ ] Al menos 2 flujos E2E críticos (atención completa y cancelación con confirmación) automatizados y documentados con comandos exactos de ejecución.

---

### Issue #13 — Auditoría de seguridad, verificación sin secretos y pruebas de calidad integral
* **Responsable sugerido:** Gael Antonio Ayuso Contreras + Cristopher Israel Cen Santana
* **Rama:** `feature/13-auditoria-seguridad`
* **Trazabilidad:** ADA-08 (`cargo audit`, `npm audit`) · `RNF-S01`–`RNF-S04`, `RD-01`
* **Objetivo:** Validar que no existan vulnerabilidades críticas en dependencias ni secretos expuestos, y verificar funcionamiento completamente offline.
* **Tareas concretas:**
  1. Ejecutar `cargo audit` en `src-tauri/` y `pnpm audit` (o `npm audit`) en raíz; resolver o justificar hallazgos.
  2. Revisar repositorio, historial reciente y `docs/` para confirmar `RNF-S02` (sin API keys, tokens ni rutas privadas de integrantes).
  3. Probar la app compilada desconectada de internet (`RNF-S03`, `RD-01`).
  4. Verificar que `cargo doc --no-deps` compile sin enlaces rotos.
* **Criterios de aceptación:**
  * [ ] Reporte de `cargo audit` y `pnpm audit` archivado/documentado en el PR.
  * [ ] Verificación exitosa sin conexión a internet.

---

### Issue #14 — Documentación final: `README.md`, matriz de trazabilidad, extensiones y revisión final (`RNF-M04`, `RNF-M05`)
* **Responsable sugerido:** Diego Gervacio de la Paz (documentación) + revisión de los 4 integrantes
* **Rama:** `docs/14-documentacion-trazabilidad`
* **Trazabilidad:** `RNF-M04`, `RNF-M05`, `RC-06`, `RC-07`, Sección 10 de ADA-09, Plan de comprobación de 3 semanas de ADA-08
* **Objetivo:** Entregar toda la documentación exigida por el ERS y cerrar los indicadores de seguimiento de ADA-08.
* **Tareas concretas:**
  1. Actualizar `README.md` con requisitos previos (Node, pnpm, Rust), instalación, ejecución (`pnpm tauri dev`), pruebas (`cargo test`, `pnpm test`, `WebdriverIO`), linters y referencia al repositorio `criscnsn/CS-ADA9-gestor-de-turnos`.
  2. Crear `docs/extensiones_ers.md` documentando `RF-13` (habilitar/inhabilitar ventanilla), la decisión de SQLite (`rusqlite`), Vista Cliente/Cajero multi-ventana y su compatibilidad con `RC-01`–`RC-07` y `CV-01`–`CV-10`.
  3. Crear `docs/trazabilidad.md` mapeando cada requisito `RF-01`–`RF-13`, `RN-01`–`RN-10`, `RD-01`–`RD-03`, `RI-01`–`RI-03`, `RNF-*` ↔ Issues/Sub-issues/PRs ↔ Pruebas que lo verifican.
  4. Crear `docs/revision_final.md` (`RNF-M05`) con oportunidades reales de mejora detectadas al revisar la v1.0 y el resumen de los **5 indicadores de ADA-08** (tiempo de setup, % PRs con pruebas, fallos detectados antes de merge, tiempos de corrección y registro de uso de IA verificado).
* **Criterios de aceptación:**
  * [ ] Un integrante (o evaluador) puede clonar el repo limpio, seguir `README.md` y ejecutar app + pruebas sin ayuda externa (`RC-07`).
  * [ ] Todos los requisitos `RF-01`–`RF-12` y casos `CV-01`–`CV-10` tienen evidencia trazable en `docs/trazabilidad.md`.
