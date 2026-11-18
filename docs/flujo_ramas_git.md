# Estrategia de Ramas y Flujo Git (Alineado a ADA-08)

Este documento define cómo organizará las ramas el equipo de 4 integrantes tomando en cuenta:
1. El flujo obligatorio declarado en **`CDS_ADA08_Equipo7.pdf`** (págs. 13–14).
2. El estado actual de la rama `main` del repositorio.

---

## 1. Respuesta directa: ¿Ramas por persona con sub-ramas, o ramas por Issue?

**Recomendación firme: NO usar ramas largas por persona (ej. `rama-cristopher`, `rama-gael`) con sub-ramas acumulándose.**

### Por qué evitar ramas por persona:
* **Contradice ADA-08:** El flujo comprometido en el PDF establece explícitamente crear ramas con el formato `feature/<issue>-descripcion` vinculadas a un issue de GitHub Projects.
* **Genera conflictos grandes al integrar (*merge hell*):** Si cada integrante acumula cambios en su propia rama durante días, al final las interfaces, el `Cargo.toml` y el `package.json` divergen y resolver conflictos consume tiempo crítico.
* **Oculta errores hasta el final:** El objetivo de ADA-08 (*"Problemas detectados antes del merge"*) requiere integrar piezas pequeñas y probadas con frecuencia.

### Modelo adoptado: **GitHub Flow (`main` + ramas efímeras `feature/<issue>-descripcion`)**
* **`main` es la única rama permanente.** Siempre debe compilar y pasar todas las pruebas y checks de CI.
* **Nadie hace commits directos a `main`** (salvo si el equipo acuerda una excepción única para inicializar reglas/plantillas en el Issue #1).
* **Cada Issue de GitHub Projects = 1 rama corta `feature/<numero>-descripcion-corta`** que nace desde `main` actualizado y regresa a `main` mediante **Pull Request (PR)**.

```text
main ──●──────────────────────●─────────────────●──────────────●───►
        \                    / \               / \            /
         feature/1-setup-ci ●   feature/2-ipc ●   feature/4  ●
                                                  (dominio)
```

> **¿Qué pasa si un Issue B depende de un Issue A que aún no se aprueba en `main`?**
> En lugar de crear una jerarquía compleja de ramas por persona:
> 1. Se intenta que el Issue A sea pequeño para mergearse rápido a `main`.
> 2. Si urge avanzar en B mientras A está en revisión, B puede ramificarse temporalmente desde A (`git checkout -b feature/5-persistencia feature/4-dominio`), y en cuanto A entra a `main`, se actualiza B con `git rebase main` (o `git merge main`) antes de abrir su PR a `main`.

---

## 2. Estado actual de `main` y punto de partida

Actualmente en `main` (último commit `f524f6d`) existen archivos preliminares vacíos:
* En `src-tauri/`: `state.rs`, `models/mod.rs`, `commands/mod.rs`, `services/mod.rs` (ubicados fuera de `src-tauri/src/`).
* En `src/`: `layouts/MainLayout.tsx`, `layouts/VentanillaLayout.tsx`, `pages/TurnosPage.tsx`, `pages/VentanillaPage.tsx`.

**Regla de arranque (Issue #1 y #2):**
* Antes de que los 4 abran ramas paralelas de features, el **Issue #1 (Setup de entorno, linters, CI y limpieza estructural)** deja `main` limpio:
  * Mueve los módulos Rust vacíos dentro de `src-tauri/src/` (con `export {}` o módulos mínimos válidos para que `cargo check` y `tsc` pasen con **0 errores**).
  * Agrega `Vitest`, `ESLint`, `Prettier`, `rustfmt`/`clippy` en CI y `.github/PULL_REQUEST_TEMPLATE.md`.
* El **Issue #2 (Contrato base en código)** agrega los tipos compartidos (`src/shared/` o `types/`) y esqueletos de firmas en Rust para que el frontend pueda avanzar sin esperar al backend completo.

---

## 3. Convención de Nombres

### Ramas
Formato oficial de ADA-08:
* `feature/<id-issue>-<descripcion-kebab-case>`
  * Ejemplos:
    * `feature/1-setup-entorno-ci`
    * `feature/3-dominio-fifo-rust`
    * `feature/5-persistencia-sqlite`
    * `feature/8-vista-cajero-ui`
* Para corrección de errores detectados en pruebas/integración:
  * `fix/<id-issue>-<descripcion-kebab-case>`
* Para documentación exclusiva:
  * `docs/<id-issue>-<descripcion-kebab-case>`

### Commits (recomendado para trazabilidad)
Usar formato claro con referencia al issue:
* `feat(#3): implementar cola FIFO y máquina de estados de turnos`
* `test(#5): agregar prueba CV-09 de fallo simulado en transacción SQLite`
* `fix(#8): mostrar ID de turno en error al intentar cancelar en atención`
* `docs(#12): actualizar matriz de trazabilidad ERS <-> issues`

---

## 4. Ciclo Diario de Trabajo por Integrante (Paso a Paso ADA-08)

1. **Tomar un Issue del tablero** en GitHub Projects y moverlo a `In Progress` (registrando hora de inicio si se mide tiempo de corrección/avance).
2. **Sincronizar `main` local:**
   ```sh
   git checkout main
   git pull origin main
   ```
3. **Crear la rama del issue:**
   ```sh
   git checkout -b feature/4-dominio-fifo-rust
   ```
4. **Implementar en VS Code** (si se usa IA como Gemini/Codex/Claude, guardar el prompt/respuesta y qué se verificó manualmente, como pide ADA-08).
5. **Validar localmente antes de subir:**
   * Backend Rust (dentro de `src-tauri/`):
     ```sh
     cargo fmt --check
     cargo clippy --all-targets -- -D warnings
     cargo test
     cargo doc --no-deps
     ```
   * Frontend (en la raíz):
     ```sh
     pnpm lint
     pnpm format:check
     pnpm test
     pnpm build
     ```
6. **Subir rama y abrir Pull Request hacia `main`:**
   ```sh
   git push -u origin feature/4-dominio-fifo-rust
   ```
   En el PR completar la plantilla:
   * `Closes #<numero-issue>`
   * Requisitos de ADA-09 cubiertos (ej. `RF-04`, `RN-02`, `CV-01`)
   * Evidencia de pruebas ejecutadas (`cargo test` / `Vitest`)
   * Registro de uso de IA (si aplicó) y verificación humana
7. **Revisión por pares (obligatorio según ADA-08):**
   * **Otro integrante distinto al autor** revisa requisitos, pruebas y uso de IA, verifica que GitHub Actions esté en verde y aprueba el PR.
8. **Integrar y limpiar:**
   * Hacer **Squash and merge** (mantiene `main` limpio con 1 commit por issue) o **Merge commit** si el equipo lo prefiere.
   * Borrar la rama remota en GitHub y en local (`git branch -d feature/4-dominio-fifo-rust`).
   * Mover el issue a `Done` en GitHub Projects.

---

## 5. Protección recomendada en GitHub (Settings → Branches)

Configurar regla de protección para `main`:
* **Require a pull request before merging** (mínimo **1 aprobación** de otro integrante).
* **Require status checks to pass before merging** (workflow de GitHub Actions con `fmt`, `clippy`, `cargo test`, `eslint`, `prettier`, `vitest`).
* **Do not allow bypassing** (para garantizar las métricas del plan de 3 semanas de ADA-08).
