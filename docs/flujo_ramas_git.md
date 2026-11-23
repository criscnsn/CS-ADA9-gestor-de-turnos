# Estrategia de Ramas y Flujo Git (Alineado a ADA-08)

> **Repositorio:** `criscnsn/CS-ADA9-gestor-de-turnos`

Este documento define cómo organizará las ramas el equipo de 4 integrantes tomando en cuenta:
1. El flujo obligatorio declarado en **`CDS_ADA08_Equipo7.pdf`** (págs. 13–14).
2. El plan **descentralizado en sub-issues** definido en [docs/issues_backlog.md](./issues_backlog.md) (`Issues_Semana1_y_Bloque2_ADA9.pdf`).
3. El estado actual de la rama `main` del repositorio.

---

## 1. Respuesta directa: ¿Ramas por persona con sub-ramas, o ramas por Issue / Sub-issue?

**Recomendación firme: NO usar ramas largas por persona (ej. `rama-cristopher`, `rama-gael`) con sub-ramas acumulándose.**

### Por qué evitar ramas por persona:
* **Contradice ADA-08:** El flujo comprometido en el PDF establece explícitamente crear ramas con el formato `feature/<issue>-descripcion` vinculadas a un issue de GitHub Projects.
* **Genera conflictos grandes al integrar (*merge hell*):** Si cada integrante acumula cambios en su propia rama durante días, al final las interfaces, el `Cargo.toml` y el `package.json` divergen y resolver conflictos consume tiempo crítico.
* **Oculta errores hasta el final:** El objetivo de ADA-08 (*"Problemas detectados antes del merge"*) requiere integrar piezas pequeñas y probadas con frecuencia.

### Modelo adoptado: **GitHub Flow (`main` + ramas efímeras por Issue / Sub-issue)**
* **`main` es la única rama permanente.** Siempre debe compilar y pasar todas las pruebas y checks de CI.
* **Nadie hace commits directos a `main`** una vez activadas las reglas de PR.
* **Cada Issue o Sub-issue = 1 rama corta `feature/<numero>[letra]-descripcion-corta`** que nace desde `main` actualizado y regresa a `main` mediante **Pull Request (PR)**.
* Dividir issues grandes en sub-issues (`#01-A/B/C`, `#02-A/B`, `#03-A/B`, `#05-A/B`, `#07-A/B`) permite trabajar en paralelo sin tocar los mismos archivos ni crear ramas personales.

```text
main ──●──────────────────────────●────────────────────────●──────────────────●───►
        \                        / \                      / \                /
         feature/1a-setup-rust  ●   feature/2a-tipos-rust ●  feature/3a-... ●
         feature/1b-frontend    ●   feature/2b-mock-ts    ●  feature/5a-sqlite...
```

> **¿Qué pasa si una Sub-issue B depende de una Sub-issue A que aún no se aprueba en `main`?**
> En lugar de crear una jerarquía compleja de ramas por persona:
> 1. Al ser sub-issues atómicas, A se revisa y mergea rápido a `main`.
> 2. Si urge avanzar en B mientras A está en revisión (por ejemplo `#05-B` respecto a `#05-A`), B puede ramificarse temporalmente desde A (`git checkout -b feature/5b-sqlite-corrupcion-tests-cv feature/5a-sqlite-esquema-transacciones`), y en cuanto A entra a `main`, se actualiza B con `git rebase main` (o `git merge main`) antes de abrir su PR a `main`.

---

## 2. Estado actual de `main` y punto de partida

* `910e61f`: Se agregó la documentación base en `docs/`.
* `0c312a4`: Se agregó la configuración inicial de herramientas ADA-08 (`ESLint`, `Prettier`, `Vitest`, `rust-toolchain.toml`, `.vscode/settings.json`, `.github/PULL_REQUEST_TEMPLATE.md` y `.github/workflows/ci.yml`).
* **Pendientes de limpieza del Bloque 0 (`#01-A` y `#01-B`):**
  * En `src-tauri/`: `state.rs`, `models/mod.rs`, `commands/mod.rs`, `services/mod.rs` siguen fuera de `src-tauri/src/` → se reubican y declaran en `src-tauri/src/lib.rs` en **`#01-A`** (`feature/1a-setup-rust-base`).
  * En `src/`: `layouts/MainLayout.tsx`, `layouts/VentanillaLayout.tsx`, `pages/TurnosPage.tsx`, `pages/VentanillaPage.tsx` siguen vacíos → se dejan con módulo válido en **`#01-B`** (`feature/1b-setup-frontend-tooling`).

---

## 3. Convención de Nombres

### Ramas
Formato oficial de ADA-08 adaptado al plan de sub-issues:
* `feature/<id-issue>[letra]-<descripcion-kebab-case>`
  * Ejemplos exactos de Semana 1 y Bloque 2:
    * `feature/1a-setup-rust-base` (GH #3)
    * `feature/1b-setup-frontend-tooling` (GH #3)
    * `feature/1c-setup-github-ci` (GH #3)
    * `feature/2a-contrato-tipos-rust`
    * `feature/2b-contrato-mock-ts`
    * `feature/3a-dominio-registro-consultas`
    * `feature/3b-dominio-transiciones-turnos`
    * `feature/4-dominio-rf13-tests-cv`
    * `feature/5a-sqlite-esquema-transacciones` (GH #2)
    * `feature/5b-sqlite-corrupcion-tests-cv` (GH #2)
    * `feature/6-ipc-commands-eventos`
    * `feature/7a-multiventana-webview`
    * `feature/7b-benchmark-200-turnos`
* Para corrección de errores detectados en pruebas/integración:
  * `fix/<id-issue>-<descripcion-kebab-case>`
* Para documentación exclusiva:
  * `docs/<id-issue>-<descripcion-kebab-case>`

### Commits (recomendado para trazabilidad)
Usar formato claro con referencia al issue o sub-issue:
* `feat(#3a): implementar registro de turnos con secuencia monotónica y consulta FIFO`
* `feat(#5a): crear esquema SQLite con rusqlite y transacciones atómicas`
* `test(#5b): agregar pruebas CV-07 a CV-09 y detección de corrupción RD-03`
* `fix(#9): mostrar ID de turno en error al intentar cancelar en atención`
* `docs(#14): actualizar matriz de trazabilidad ERS <-> issues`

---

## 4. Ciclo Diario de Trabajo por Integrante (Paso a Paso ADA-08)

1. **Tomar una Issue / Sub-issue del tablero** en GitHub Projects y moverla a `In Progress` (registrando hora de inicio si se mide tiempo de corrección/avance).
2. **Sincronizar `main` local:**
   ```sh
   git checkout main
   git pull origin main
   ```
3. **Crear la rama de la sub-issue:**
   ```sh
   git checkout -b feature/3a-dominio-registro-consultas
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
   git push -u origin feature/3a-dominio-registro-consultas
   ```
   En el PR completar la plantilla (`.github/PULL_REQUEST_TEMPLATE.md`):
   * `Closes #<numero-issue-github>` (o referencia a la issue padre si es sub-issue)
   * Requisitos de ADA-09 cubiertos (ej. `RF-02`, `RN-02`, `CV-01`)
   * Evidencia de pruebas ejecutadas (`cargo test` / `Vitest`)
   * Registro de uso de IA (si aplicó) y verificación humana
7. **Revisión por pares (obligatorio según ADA-08):**
   * **Otro integrante distinto al autor** revisa requisitos, pruebas y uso de IA, verifica que GitHub Actions esté en verde y aprueba el PR.
8. **Integrar y limpiar:**
   * Hacer **Squash and merge** (mantiene `main` limpio con 1 commit por sub-issue) o **Merge commit** si el equipo lo prefiere.
   * Borrar la rama remota en GitHub y en local (`git branch -d feature/3a-dominio-registro-consultas`).
   * Mover la sub-issue a `Done` en GitHub Projects.

---

## 5. Protección recomendada en GitHub (Settings → Branches)

Configurar regla de protección para `main`:
* **Require a pull request before merging** (mínimo **1 aprobación** de otro integrante).
* **Require status checks to pass before merging** (workflow de GitHub Actions con `fmt`, `clippy`, `cargo test`, `eslint`, `prettier`, `vitest`).
* **Do not allow bypassing** (para garantizar las métricas del plan de 3 semanas de ADA-08).
