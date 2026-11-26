//! Transiciones de estado de atención del Sistema de Gestión de Turnos (SGT).
//!
//! Implementa las operaciones mutadoras del ciclo de vida de un turno sobre
//! [`SistemaTurnos`] sin acoplamiento a Tauri ni a SQLite (`RNF-M01`):
//! - [`SistemaTurnos::asignar_siguiente`] (`RF-04`, `RF-05`, `RN-01`, `RN-02`,
//!   `RN-04`, `RN-05`, `RN-06`, `RN-10` y guard `RF-13`).
//! - [`SistemaTurnos::finalizar_atencion`] (`RF-06`, `RF-07`, `RN-08`, `RN-09`).
//! - [`SistemaTurnos::cancelar_turno`] (`RF-08`, `RF-09`, `RN-07`, `RN-09`,
//!   `RNF-S04`).
//!
//! Toda operación rechazada preserva intacto el estado previo (`RNF-R02`) y
//! devuelve un [`AppError`] descriptivo con el identificador del recurso
//! implicado cuando aplica (`RNF-U02`).

use crate::domain::SistemaTurnos;
use crate::models::{AppError, CodigoError, EstadoTurno, Turno, Ventanilla, VentanillaId};

/// Devuelve la etiqueta textual estable de una [`VentanillaId`] (`"V01"`, `"V02"` o `"V03"`).
#[must_use]
fn etiqueta_ventanilla(id: VentanillaId) -> &'static str {
    match id {
        VentanillaId::V01 => "V01",
        VentanillaId::V02 => "V02",
        VentanillaId::V03 => "V03",
    }
}

/// Verifica que una ventanilla esté habilitada para recibir atención (`RF-13`).
///
/// Se expone como guard reutilizable para coordinar con `#10` sin duplicar ni
/// reescribir la validación en [`SistemaTurnos::asignar_siguiente`].
///
/// # Errors
///
/// Devuelve [`CodigoError::VentanillaInhabilitada`] si `ventanilla.habilitada`
/// es `false`.
pub fn validar_ventanilla_habilitada(ventanilla: &Ventanilla) -> Result<(), AppError> {
    if !ventanilla.habilitada {
        let etiqueta = etiqueta_ventanilla(ventanilla.id);
        return Err(AppError {
            codigo: CodigoError::VentanillaInhabilitada,
            mensaje: format!(
                "La ventanilla {etiqueta} está inhabilitada y no puede recibir turnos."
            ),
            recurso_id: Some(etiqueta.to_string()),
        });
    }
    Ok(())
}

impl SistemaTurnos {
    /// Asigna a `ventanilla_id` el turno en [`EstadoTurno::EnEspera`] con menor
    /// [`Turno::secuencia`], iniciando de inmediato su atención (`RF-04`, `RN-02`,
    /// `RN-06`).
    ///
    /// # Errors
    ///
    /// Preserva el estado sin cambios (`RNF-R02`) y devuelve:
    /// - [`CodigoError::VentanillaInhabilitada`] si la ventanilla está
    ///   inhabilitada (`RF-13`).
    /// - [`CodigoError::VentanillaOcupada`] si la ventanilla ya atiende un
    ///   turno (`RF-05`, `RN-04`).
    /// - [`CodigoError::FilaVacia`] si no existen turnos en espera (`RF-05`,
    ///   `RN-10`).
    pub fn asignar_siguiente(&mut self, ventanilla_id: VentanillaId) -> Result<Turno, AppError> {
        let etiqueta = etiqueta_ventanilla(ventanilla_id);
        let ventanilla = self.ventanilla(ventanilla_id);

        validar_ventanilla_habilitada(ventanilla)?;

        if let Some(turno_actual_id) = &ventanilla.turno_actual_id {
            return Err(AppError {
                codigo: CodigoError::VentanillaOcupada,
                mensaje: format!(
                    "La ventanilla {etiqueta} está ocupada atendiendo el turno {turno_actual_id}."
                ),
                recurso_id: Some(etiqueta.to_string()),
            });
        }

        let Some(turno) = self.siguiente_en_espera_mut() else {
            return Err(AppError {
                codigo: CodigoError::FilaVacia,
                mensaje: format!(
                    "No hay turnos en espera para asignar en la ventanilla {etiqueta}."
                ),
                recurso_id: Some(etiqueta.to_string()),
            });
        };

        turno.estado = EstadoTurno::EnAtencion;
        turno.ventanilla_asignada = Some(ventanilla_id);
        let turno_asignado = turno.clone();

        self.ventanilla_mut(ventanilla_id).turno_actual_id = Some(turno_asignado.id.clone());

        Ok(turno_asignado)
    }

    /// Finaliza el turno actualmente en [`EstadoTurno::EnAtencion`] en
    /// `ventanilla_id`, pasándolo a [`EstadoTurno::Atendido`] y liberando la
    /// ventanilla (`RF-06`, `RN-08`, `RN-09`).
    ///
    /// El turno conserva su [`Turno::ventanilla_asignada`] para la consulta de
    /// historial (`RF-10`).
    ///
    /// # Errors
    ///
    /// Preserva el estado sin cambios (`RNF-R02`) y devuelve:
    /// - [`CodigoError::VentanillaLibre`] si la ventanilla no tiene un turno
    ///   en atención (`RF-07`).
    /// - [`CodigoError::BaseDatosCorrupta`] si existiera una referencia interna
    ///   inconsistente entre la ventanilla y el turno referenciado.
    pub fn finalizar_atencion(&mut self, ventanilla_id: VentanillaId) -> Result<Turno, AppError> {
        let etiqueta = etiqueta_ventanilla(ventanilla_id);
        let Some(turno_actual_id) = self.ventanilla(ventanilla_id).turno_actual_id.clone() else {
            return Err(AppError {
                codigo: CodigoError::VentanillaLibre,
                mensaje: format!(
                    "La ventanilla {etiqueta} está libre y no tiene ningún turno en atención para finalizar."
                ),
                recurso_id: Some(etiqueta.to_string()),
            });
        };

        let Some(turno) = self.buscar_turno_mut(&turno_actual_id) else {
            return Err(AppError {
                codigo: CodigoError::BaseDatosCorrupta,
                mensaje: format!(
                    "La ventanilla {etiqueta} referencia el turno inexistente {turno_actual_id}."
                ),
                recurso_id: Some(turno_actual_id),
            });
        };

        if turno.estado != EstadoTurno::EnAtencion {
            return Err(AppError {
                codigo: CodigoError::BaseDatosCorrupta,
                mensaje: format!(
                    "El turno {turno_actual_id} asignado a {etiqueta} no se encuentra en estado En atención."
                ),
                recurso_id: Some(turno_actual_id),
            });
        }

        turno.estado = EstadoTurno::Atendido;
        let turno_finalizado = turno.clone();

        self.ventanilla_mut(ventanilla_id).turno_actual_id = None;

        Ok(turno_finalizado)
    }

    /// Cancela un turno pendiente cuyo estado sea [`EstadoTurno::EnEspera`],
    /// cambiándolo a [`EstadoTurno::Cancelado`] y conservándolo en el historial
    /// (`RF-08`, `RN-07`, `RN-09`).
    ///
    /// Normaliza espacios laterales en `turno_id` antes de buscarlo y rechaza
    /// entradas vacías o con caracteres de control (`RNF-S04`).
    ///
    /// # Errors
    ///
    /// Preserva el estado sin cambios (`RNF-R02`) y devuelve:
    /// - [`CodigoError::EntradaInvalida`] si `turno_id` está vacío o contiene
    ///   caracteres de control (`RNF-S04`).
    /// - [`CodigoError::TurnoInexistente`] si no existe un turno con ese
    ///   identificador (`RF-09`).
    /// - [`CodigoError::EstadoTurnoInvalido`] si el turno no está en
    ///   [`EstadoTurno::EnEspera`] (`RF-09`, `RN-07`).
    pub fn cancelar_turno(&mut self, turno_id: &str) -> Result<Turno, AppError> {
        if turno_id.chars().any(char::is_control) {
            return Err(AppError {
                codigo: CodigoError::EntradaInvalida,
                mensaje: "El identificador de turno contiene caracteres de control no permitidos."
                    .to_string(),
                recurso_id: None,
            });
        }
        let id = turno_id.trim();
        if id.is_empty() {
            return Err(AppError {
                codigo: CodigoError::EntradaInvalida,
                mensaje: "El identificador de turno debe ser un texto válido y no vacío."
                    .to_string(),
                recurso_id: None,
            });
        }

        let Some(turno) = self.buscar_turno_mut(id) else {
            return Err(AppError {
                codigo: CodigoError::TurnoInexistente,
                mensaje: format!("El turno {id} no existe."),
                recurso_id: Some(id.to_string()),
            });
        };

        if turno.estado != EstadoTurno::EnEspera {
            let descripcion_estado = match turno.estado {
                EstadoTurno::EnAtencion => "actualmente se encuentra En atención",
                EstadoTurno::Atendido => "ya fue Atendido",
                EstadoTurno::Cancelado => "ya se encuentra Cancelado",
                EstadoTurno::EnEspera => unreachable!(),
            };
            return Err(AppError {
                codigo: CodigoError::EstadoTurnoInvalido,
                mensaje: format!("No se puede cancelar el turno {id} porque {descripcion_estado}."),
                recurso_id: Some(id.to_string()),
            });
        }

        turno.estado = EstadoTurno::Cancelado;
        Ok(turno.clone())
    }
}
