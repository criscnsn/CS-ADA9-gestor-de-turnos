//! Extensión `RF-13`: habilitar e inhabilitar ventanillas sobre [`SistemaTurnos`].
//!
//! Invariantes que protege este módulo:
//!
//! - Una ventanilla ocupada (con turno `EN_ATENCION`) no se puede inhabilitar.
//! - Siempre queda al menos una ventanilla habilitada.
//!
//! El rechazo de asignaciones en ventanillas inhabilitadas es la misma regla y
//! vive en un único lugar: [`crate::domain::transiciones::validar_ventanilla_habilitada`],
//! que reutiliza [`SistemaTurnos::asignar_siguiente`].
//!
//! SQLite aplica estas reglas como segunda barrera (trigger
//! `trg_ventanillas_minimo_una_activa` y `CHECK` de `001_initial.sql`), pero el
//! dominio las valida primero para devolver un [`AppError`] claro.

use crate::domain::SistemaTurnos;
use crate::models::{AppError, CodigoError, Ventanilla, VentanillaId};

impl SistemaTurnos {
    /// Habilita o inhabilita `ventanilla_id` (`RF-13`).
    ///
    /// Valida antes de mutar: si devuelve error, el estado queda intacto
    /// (`RNF-R02`). Pedir el estado que la ventanilla ya tiene es válido y no
    /// cambia nada. Habilitar siempre se permite.
    ///
    /// # Errors
    ///
    /// - [`CodigoError::VentanillaOcupada`] al inhabilitar una ventanilla con
    ///   turno en atención.
    /// - [`CodigoError::UltimaVentanillaActiva`] al inhabilitar la única
    ///   ventanilla habilitada.
    pub fn cambiar_estado_ventanilla(
        &mut self,
        ventanilla_id: VentanillaId,
        habilitada: bool,
    ) -> Result<Ventanilla, AppError> {
        let otras_habilitadas = self
            .consultar_ventanillas()
            .iter()
            .filter(|v| v.id != ventanilla_id && v.habilitada)
            .count();
        let ventanilla = self.ventanilla_mut(ventanilla_id);

        if !habilitada && ventanilla.habilitada {
            if let Some(turno_actual_id) = &ventanilla.turno_actual_id {
                return Err(AppError {
                    codigo: CodigoError::VentanillaOcupada,
                    mensaje: format!(
                        "No se puede inhabilitar la ventanilla {ventanilla_id:?}: está atendiendo el turno {turno_actual_id}."
                    ),
                    recurso_id: Some(format!("{ventanilla_id:?}")),
                });
            }
            if otras_habilitadas == 0 {
                return Err(AppError {
                    codigo: CodigoError::UltimaVentanillaActiva,
                    mensaje: format!(
                        "No se puede inhabilitar la ventanilla {ventanilla_id:?}: debe quedar al menos una ventanilla habilitada."
                    ),
                    recurso_id: Some(format!("{ventanilla_id:?}")),
                });
            }
        }

        ventanilla.habilitada = habilitada;
        Ok(ventanilla.clone())
    }
}
