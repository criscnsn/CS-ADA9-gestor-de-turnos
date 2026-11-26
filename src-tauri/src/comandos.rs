//! Comandos IPC del SGT, estado gestionado y evento en tiempo real (`#14`).
//!
//! Orquesta cada operación mutadora con el patrón estricto de `RD-02` / `RF-12`:
//!
//! 1. El dominio ([`SistemaTurnos`]) aplica la operación sobre un borrador y
//!    valida las reglas de negocio (aquí no se duplican validaciones).
//! 2. El [`Repositorio`] guarda solo lo que cambió en una transacción.
//! 3. Únicamente si el commit fue exitoso se publica el borrador en memoria y se
//!    emite [`EVENTO_ESTADO_ACTUALIZADO`]. Si falla, la memoria queda intacta y no
//!    se emite nada.

use std::sync::{Arc, Mutex, PoisonError};

use tauri::{AppHandle, Emitter, State};

use crate::domain::SistemaTurnos;
use crate::models::{AppError, CodigoError, EstadoSistema, Turno, Ventanilla, VentanillaId};

/// Evento que reciben todas las ventanas tras cada mutación confirmada (`RI-02`).
pub const EVENTO_ESTADO_ACTUALIZADO: &str = "sgt://estado-actualizado";

/// Puerto de persistencia que necesita el cableado IPC; lo implementa `#12`.
pub trait Repositorio: Send {
    /// Carga el estado persistido al arrancar (vía [`SistemaTurnos::restaurar`]).
    ///
    /// # Errors
    ///
    /// [`CodigoError::BaseDatosCorrupta`] si la base de datos está dañada, sin
    /// sobrescribirla (`RD-03`).
    fn cargar(&mut self) -> Result<SistemaTurnos, AppError>;

    /// Guarda `cambios` en una única transacción `BEGIN IMMEDIATE ... COMMIT`.
    ///
    /// # Errors
    ///
    /// [`CodigoError::ErrorPersistencia`] tras hacer `ROLLBACK` ante cualquier fallo.
    fn guardar(&mut self, cambios: &Cambios) -> Result<(), AppError>;
}

/// Filas que una operación modificó y que el repositorio debe persistir.
///
/// Solo incluye turnos nuevos o modificados: el trigger
/// `trg_turnos_estados_terminales` rechaza cualquier `UPDATE` sobre turnos
/// `ATENDIDO` o `CANCELADO`, aunque no cambie nada.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cambios {
    /// Turnos nuevos o modificados (`INSERT ... ON CONFLICT(id) DO UPDATE`).
    pub turnos: Vec<Turno>,
    /// Ventanillas modificadas.
    pub ventanillas: Vec<Ventanilla>,
    /// Valor de `secuencias.ultimo_valor` tras la operación.
    pub ultima_secuencia: u64,
}

/// Estado del backend: memoria de dominio y su repositorio.
pub struct Sgt {
    sistema: SistemaTurnos,
    repo: Box<dyn Repositorio>,
}

/// Estado gestionado por Tauri. Guarda el error de arranque (p. ej.
/// `BASE_DATOS_CORRUPTA`) para que cada comando lo informe a la interfaz.
pub type EstadoApp = Arc<Mutex<Result<Sgt, AppError>>>;

impl Sgt {
    /// Carga el estado inicial desde `repo`.
    ///
    /// # Errors
    ///
    /// Propaga el error de [`Repositorio::cargar`].
    pub fn iniciar(mut repo: Box<dyn Repositorio>) -> Result<Self, AppError> {
        let sistema = repo.cargar()?;
        Ok(Self { sistema, repo })
    }

    /// Snapshot actual para `obtener_estado` y el evento.
    #[must_use]
    pub fn obtener_estado(&self) -> EstadoSistema {
        self.sistema.obtener_estado()
    }

    /// Aplica `operacion` con el patrón commit → memoria → evento.
    ///
    /// # Errors
    ///
    /// Devuelve el error del dominio o del repositorio; en ambos casos la
    /// memoria no cambia y `emitir` no se llama (`RNF-R02`).
    pub fn ejecutar<T>(
        &mut self,
        operacion: impl FnOnce(&mut SistemaTurnos) -> Result<T, AppError>,
        emitir: impl FnOnce(EstadoSistema),
    ) -> Result<T, AppError> {
        // ponytail: clona el estado completo por operación; basta para RNF-P01
        // (200 turnos). Si crece mucho, aplicar la operación con deshacer.
        let mut borrador = self.sistema.clone();
        let resultado = operacion(&mut borrador)?;
        self.repo.guardar(&cambios(&self.sistema, &borrador))?;
        self.sistema = borrador;
        emitir(self.sistema.obtener_estado());
        Ok(resultado)
    }
}

/// Diferencia entre dos estados. Los turnos nunca se borran ni se reordenan
/// (`RN-09`), así que se comparan por posición.
fn cambios(antes: &SistemaTurnos, despues: &SistemaTurnos) -> Cambios {
    let turnos_antes = antes.turnos_slice();
    Cambios {
        turnos: despues
            .turnos_slice()
            .iter()
            .enumerate()
            .filter(|(i, turno)| turnos_antes.get(*i) != Some(turno))
            .map(|(_, turno)| turno.clone())
            .collect(),
        ventanillas: despues
            .consultar_ventanillas()
            .iter()
            .zip(antes.consultar_ventanillas())
            .filter(|(nueva, vieja)| nueva != vieja)
            .map(|(nueva, _)| nueva.clone())
            .collect(),
        ultima_secuencia: despues.ultima_secuencia(),
    }
}

/// Bloquea el estado gestionado y ejecuta `f` sobre el backend iniciado.
fn con_sgt<T>(
    estado: &EstadoApp,
    f: impl FnOnce(&mut Sgt) -> Result<T, AppError>,
) -> Result<T, AppError> {
    // Un pánico a mitad de operación no deja memoria a medias: solo se publica
    // tras el commit. Por eso es seguro recuperar un mutex envenenado.
    let mut guardia = estado.lock().unwrap_or_else(PoisonError::into_inner);
    let sgt = guardia.as_mut().map_err(|err| err.clone())?;
    f(sgt)
}

/// Ejecuta una operación mutadora y emite el evento con `app` tras el commit.
fn mutar<T>(
    app: &AppHandle,
    estado: &EstadoApp,
    operacion: impl FnOnce(&mut SistemaTurnos) -> Result<T, AppError>,
) -> Result<T, AppError> {
    con_sgt(estado, |sgt| {
        sgt.ejecutar(operacion, |snapshot| {
            // Los datos ya están confirmados; un fallo al notificar no los revierte.
            if let Err(err) = app.emit(EVENTO_ESTADO_ACTUALIZADO, snapshot) {
                eprintln!("No se pudo emitir {EVENTO_ESTADO_ACTUALIZADO}: {err}");
            }
        })
    })
}

/// Sanea (`trim`) y valida un identificador de ventanilla (`RNF-S04`).
fn parsear_ventanilla(id: &str) -> Result<VentanillaId, AppError> {
    match id.trim() {
        "V01" => Ok(VentanillaId::V01),
        "V02" => Ok(VentanillaId::V02),
        "V03" => Ok(VentanillaId::V03),
        otro => Err(AppError {
            codigo: CodigoError::VentanillaInvalida,
            mensaje: format!("La ventanilla '{otro}' no existe. Use V01, V02 o V03."),
            recurso_id: Some(otro.to_string()),
        }),
    }
}

/// Devuelve ventanillas, fila FIFO e historial (`RF-01`, `RF-03`, `RF-10`).
///
/// # Errors
///
/// Error de arranque guardado en el estado (p. ej. `BASE_DATOS_CORRUPTA`).
#[tauri::command]
pub fn obtener_estado(estado: State<'_, EstadoApp>) -> Result<EstadoSistema, AppError> {
    con_sgt(&estado, |sgt| Ok(sgt.obtener_estado()))
}

/// Registra un turno nuevo en espera (`RF-02`).
///
/// # Errors
///
/// Error del dominio o de persistencia.
#[tauri::command]
pub fn registrar_turno(app: AppHandle, estado: State<'_, EstadoApp>) -> Result<Turno, AppError> {
    mutar(&app, &estado, SistemaTurnos::registrar_turno)
}

/// Asigna a la ventanilla el siguiente turno FIFO (`RF-04`, `RF-05`, `RF-13`).
///
/// # Errors
///
/// `VENTANILLA_INVALIDA`, errores del dominio o de persistencia.
#[tauri::command]
pub fn asignar_siguiente(
    app: AppHandle,
    estado: State<'_, EstadoApp>,
    ventanilla_id: String,
) -> Result<Turno, AppError> {
    let id = parsear_ventanilla(&ventanilla_id)?;
    mutar(&app, &estado, |s| s.asignar_siguiente(id))
}

/// Finaliza el turno en atención de la ventanilla (`RF-06`, `RF-07`).
///
/// # Errors
///
/// `VENTANILLA_INVALIDA`, errores del dominio o de persistencia.
#[tauri::command]
pub fn finalizar_atencion(
    app: AppHandle,
    estado: State<'_, EstadoApp>,
    ventanilla_id: String,
) -> Result<Turno, AppError> {
    let id = parsear_ventanilla(&ventanilla_id)?;
    mutar(&app, &estado, |s| s.finalizar_atencion(id))
}

/// Cancela un turno en espera (`RF-08`, `RF-09`). El dominio sanea el ID.
///
/// # Errors
///
/// Errores del dominio o de persistencia.
#[tauri::command]
pub fn cancelar_turno(
    app: AppHandle,
    estado: State<'_, EstadoApp>,
    turno_id: String,
) -> Result<Turno, AppError> {
    mutar(&app, &estado, |s| s.cancelar_turno(&turno_id))
}

/// Habilita o inhabilita una ventanilla (`RF-13`).
///
/// # Errors
///
/// `VENTANILLA_INVALIDA`, errores del dominio o de persistencia.
#[tauri::command]
pub fn cambiar_estado_ventanilla(
    app: AppHandle,
    estado: State<'_, EstadoApp>,
    ventanilla_id: String,
    habilitada: bool,
) -> Result<Ventanilla, AppError> {
    let id = parsear_ventanilla(&ventanilla_id)?;
    mutar(&app, &estado, |s| {
        s.cambiar_estado_ventanilla(id, habilitada)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::EstadoTurno;
    use VentanillaId::{V01, V03};

    /// Lo que el repositorio doble recibió, compartido con la prueba.
    #[derive(Default)]
    struct Registro {
        fallar: bool,
        guardados: Vec<Cambios>,
    }

    /// Repositorio doble en memoria; solo para pruebas.
    struct RepoDoble(Arc<Mutex<Registro>>);

    impl Repositorio for RepoDoble {
        fn cargar(&mut self) -> Result<SistemaTurnos, AppError> {
            Ok(SistemaTurnos::nuevo())
        }

        fn guardar(&mut self, cambios: &Cambios) -> Result<(), AppError> {
            let mut registro = self.0.lock().unwrap();
            if registro.fallar {
                return Err(AppError {
                    codigo: CodigoError::ErrorPersistencia,
                    mensaje: "Fallo simulado de escritura.".into(),
                    recurso_id: None,
                });
            }
            registro.guardados.push(cambios.clone());
            Ok(())
        }
    }

    fn sgt_con_doble() -> (Sgt, Arc<Mutex<Registro>>) {
        let registro = Arc::new(Mutex::new(Registro::default()));
        let sgt = Sgt::iniciar(Box::new(RepoDoble(registro.clone()))).expect("iniciar");
        (sgt, registro)
    }

    /// Ejecuta `operacion` y devuelve su resultado junto a los eventos emitidos.
    fn ejecutar_y_capturar<T>(
        sgt: &mut Sgt,
        operacion: impl FnOnce(&mut SistemaTurnos) -> Result<T, AppError>,
    ) -> (Result<T, AppError>, Vec<EstadoSistema>) {
        let mut eventos = Vec::new();
        let resultado = sgt.ejecutar(operacion, |snapshot| eventos.push(snapshot));
        (resultado, eventos)
    }

    #[test]
    fn commit_exitoso_publica_memoria_y_emite_un_evento() {
        let (mut sgt, registro) = sgt_con_doble();

        let (turno, eventos) = ejecutar_y_capturar(&mut sgt, SistemaTurnos::registrar_turno);

        let turno = turno.expect("registrar");
        assert_eq!(
            sgt.obtener_estado().fila_espera,
            std::slice::from_ref(&turno)
        );
        assert_eq!(eventos, [sgt.obtener_estado()]);
        let guardados = &registro.lock().unwrap().guardados;
        assert_eq!(guardados.len(), 1);
        assert_eq!(guardados[0].turnos, [turno]);
        assert_eq!(guardados[0].ultima_secuencia, 1);
    }

    #[test]
    fn fallo_de_persistencia_deja_memoria_intacta_y_sin_evento() {
        let (mut sgt, registro) = sgt_con_doble();
        registro.lock().unwrap().fallar = true;
        let antes = sgt.obtener_estado();

        let (resultado, eventos) = ejecutar_y_capturar(&mut sgt, SistemaTurnos::registrar_turno);

        assert_eq!(
            resultado.unwrap_err().codigo,
            CodigoError::ErrorPersistencia
        );
        assert_eq!(sgt.obtener_estado(), antes);
        assert!(eventos.is_empty());
    }

    #[test]
    fn rechazo_del_dominio_no_llama_al_repositorio_ni_emite() {
        let (mut sgt, registro) = sgt_con_doble();

        let (resultado, eventos) = ejecutar_y_capturar(&mut sgt, |s| s.asignar_siguiente(V01));

        assert_eq!(resultado.unwrap_err().codigo, CodigoError::FilaVacia);
        assert!(registro.lock().unwrap().guardados.is_empty());
        assert!(eventos.is_empty());
    }

    #[test]
    fn solo_se_guardan_las_filas_que_cambiaron() {
        let (mut sgt, registro) = sgt_con_doble();
        for _ in 0..2 {
            sgt.ejecutar(SistemaTurnos::registrar_turno, |_| {})
                .expect("registrar");
        }
        sgt.ejecutar(|s| s.asignar_siguiente(V01), |_| {})
            .expect("asignar");
        sgt.ejecutar(|s| s.finalizar_atencion(V01), |_| {})
            .expect("finalizar");
        sgt.ejecutar(|s| s.cambiar_estado_ventanilla(V03, false), |_| {})
            .expect("inhabilitar");

        let guardados = &registro.lock().unwrap().guardados;
        let asignar = &guardados[2];
        assert_eq!(asignar.turnos.len(), 1);
        assert_eq!(asignar.turnos[0].id, "T-0001");
        assert_eq!(asignar.turnos[0].estado, EstadoTurno::EnAtencion);
        assert_eq!(asignar.ventanillas.len(), 1);
        assert_eq!(asignar.ventanillas[0].id, V01);

        // T-0001 ya es ATENDIDO: no se reenvía (el trigger rechazaría el UPDATE).
        let inhabilitar = &guardados[4];
        assert!(inhabilitar.turnos.is_empty());
        assert_eq!(inhabilitar.ventanillas.len(), 1);
        assert_eq!(inhabilitar.ventanillas[0].id, V03);
        assert!(!inhabilitar.ventanillas[0].habilitada);
    }

    #[test]
    fn parsear_ventanilla_sanea_y_rechaza_ids_invalidos() {
        assert_eq!(parsear_ventanilla(" V02 ").unwrap(), VentanillaId::V02);
        for invalido in ["V9", "v01", ""] {
            let err = parsear_ventanilla(invalido).unwrap_err();
            assert_eq!(err.codigo, CodigoError::VentanillaInvalida);
        }
    }

    #[test]
    fn error_de_arranque_se_devuelve_en_cada_comando() {
        let corrupta = AppError {
            codigo: CodigoError::BaseDatosCorrupta,
            mensaje: "BD dañada.".into(),
            recurso_id: None,
        };
        let estado: EstadoApp = Arc::new(Mutex::new(Err(corrupta.clone())));

        assert_eq!(
            con_sgt(&estado, |sgt| Ok(sgt.obtener_estado())),
            Err(corrupta)
        );
    }
}
