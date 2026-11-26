//! Consultas puras del Sistema de Gestión de Turnos (SGT).
//!
//! Implementa:
//! - `RF-01`: consulta de las tres ventanillas (`V01`, `V02`, `V03`).
//! - `RF-03`: consulta de la fila de turnos en [`EstadoTurno::EnEspera`]
//!   ordenados ascendentemente por [`Turno::secuencia`] (`RN-02`).
//! - `RF-10`: consulta del historial completo conservando los cuatro estados.
//! - Snapshot [`EstadoSistema`] para `obtener_estado` y eventos IPC.

use crate::domain::SistemaTurnos;
use crate::models::{EstadoSistema, EstadoTurno, Turno, Ventanilla, VentanillaId};

impl SistemaTurnos {
    /// Devuelve las tres ventanillas fijas (`V01`, `V02`, `V03`) en orden
    /// canónico (`RF-01`, `RN-03`).
    #[must_use]
    pub fn consultar_ventanillas(&self) -> &[Ventanilla; 3] {
        &self.ventanillas
    }

    /// Devuelve el estado actual de una ventanilla específica (`RF-01`).
    #[must_use]
    pub fn ventanilla(&self, id: VentanillaId) -> &Ventanilla {
        let indice = Self::indice_ventanilla(id);
        &self.ventanillas[indice]
    }

    /// Devuelve los turnos en [`EstadoTurno::EnEspera`] ordenados de forma
    /// ascendente por [`Turno::secuencia`] (`RF-03`, `RN-01`, `RN-02`).
    ///
    /// El orden nunca depende del identificador alfabético ni de la precisión
    /// del reloj de registro.
    #[must_use]
    pub fn consultar_fila(&self) -> Vec<Turno> {
        let mut fila: Vec<Turno> = self
            .turnos
            .iter()
            .filter(|turno| turno.estado == EstadoTurno::EnEspera)
            .cloned()
            .collect();
        fila.sort_by_key(|turno| turno.secuencia);
        fila
    }

    /// Devuelve todos los turnos registrados en cualquiera de sus cuatro
    /// estados (`EnEspera`, `EnAtencion`, `Atendido`, `Cancelado`) ordenados
    /// ascendentemente por [`Turno::secuencia`] (`RF-10`, `RN-09`).
    #[must_use]
    pub fn consultar_historial(&self) -> Vec<Turno> {
        let mut historial = self.turnos.clone();
        historial.sort_by_key(|turno| turno.secuencia);
        historial
    }

    /// Busca un turno por su identificador exacto dentro del historial.
    #[must_use]
    pub fn buscar_turno(&self, turno_id: &str) -> Option<&Turno> {
        self.turnos.iter().find(|turno| turno.id == turno_id)
    }

    /// Construye el snapshot [`EstadoSistema`] definido en `docs/contrato_base.md`
    /// combinando ventanillas, fila FIFO ordenada e historial completo.
    #[must_use]
    pub fn obtener_estado(&self) -> EstadoSistema {
        EstadoSistema {
            ventanillas: self.ventanillas.clone(),
            fila_espera: self.consultar_fila(),
            historial: self.consultar_historial(),
        }
    }
}
