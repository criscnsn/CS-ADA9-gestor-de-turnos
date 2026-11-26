//! Registro de turnos con disciplina FIFO y secuencia monotónica creciente.
//!
//! Implementa `RF-02`, `RN-01` y `RN-02`: todo turno nuevo se incorpora al
//! sistema con identificador único (`T-0001`), secuencia estrictamente
//! creciente, fecha/hora ISO-8601 UTC, estado [`EstadoTurno::EnEspera`] y sin
//! ventanilla asignada (`None`).

use chrono::{DateTime, SecondsFormat, Utc};

use crate::domain::SistemaTurnos;
use crate::models::{AppError, CodigoError, EstadoTurno, Turno};

/// Valor máximo de secuencia representable con exactitud por `number` en
/// TypeScript (`Number.MAX_SAFE_INTEGER`).
pub const MAX_SECUENCIA_JS: u64 = 9_007_199_254_740_991;

/// Formatea un número de secuencia en el identificador legible del contrato
/// (`T-0001`, `T-0002`, ..., `T-10000`).
#[must_use]
pub fn formatear_id_turno(secuencia: u64) -> String {
    format!("T-{secuencia:04}")
}

/// Formatea una marca de tiempo UTC en formato ISO-8601 con milisegundos y
/// sufijo `Z` (p. ej. `2026-11-18T01:00:00.000Z`), consistente con
/// `Date.prototype.toISOString()` en TypeScript.
#[must_use]
pub fn formatear_fecha_iso8601(fecha_hora: DateTime<Utc>) -> String {
    fecha_hora.to_rfc3339_opts(SecondsFormat::Millis, true)
}

impl SistemaTurnos {
    /// Calcula la siguiente secuencia monotónica disponible sin mutar el estado.
    ///
    /// # Errors
    ///
    /// Devuelve [`CodigoError::EntradaInvalida`] si se superaría
    /// [`MAX_SECUENCIA_JS`].
    pub fn siguiente_secuencia(&self) -> Result<u64, AppError> {
        let siguiente = self
            .ultima_secuencia
            .checked_add(1)
            .filter(|&valor| valor <= MAX_SECUENCIA_JS)
            .ok_or_else(|| AppError {
                codigo: CodigoError::EntradaInvalida,
                mensaje: "Se alcanzó el límite máximo de secuencias admitidas para turnos."
                    .to_string(),
                recurso_id: None,
            })?;
        Ok(siguiente)
    }

    /// Registra un nuevo turno utilizando el reloj UTC actual del sistema (`RF-02`).
    ///
    /// # Errors
    ///
    /// Devuelve [`CodigoError::EntradaInvalida`] si se agotó el rango seguro de
    /// secuencias ([`MAX_SECUENCIA_JS`]).
    pub fn registrar_turno(&mut self) -> Result<Turno, AppError> {
        self.registrar_turno_en(Utc::now())
    }

    /// Registra un nuevo turno utilizando la fecha y hora UTC proporcionada.
    ///
    /// Permite pruebas deterministas sin depender del reloj del sistema y
    /// garantiza que el orden FIFO dependa únicamente de [`Turno::secuencia`]
    /// (`RN-02`).
    ///
    /// # Errors
    ///
    /// Devuelve [`CodigoError::EntradaInvalida`] si se agotó el rango seguro de
    /// secuencias ([`MAX_SECUENCIA_JS`]).
    pub fn registrar_turno_en(&mut self, fecha_hora: DateTime<Utc>) -> Result<Turno, AppError> {
        let secuencia = self.siguiente_secuencia()?;
        self.registrar_turno_con_secuencia(secuencia, fecha_hora)
    }

    /// Registra un nuevo turno usando una secuencia ya reservada por el contador
    /// transaccional del repositorio (`#12`), evitando mantener dos contadores
    /// independientes.
    ///
    /// # Errors
    ///
    /// Devuelve [`CodigoError::EntradaInvalida`] si `secuencia` es `0`, supera
    /// [`MAX_SECUENCIA_JS`] o no es estrictamente mayor que
    /// [`Self::ultima_secuencia`].
    pub fn registrar_turno_con_secuencia(
        &mut self,
        secuencia: u64,
        fecha_hora: DateTime<Utc>,
    ) -> Result<Turno, AppError> {
        if secuencia == 0 || secuencia > MAX_SECUENCIA_JS {
            return Err(AppError {
                codigo: CodigoError::EntradaInvalida,
                mensaje: format!(
                    "La secuencia de turno ({secuencia}) está fuera del rango válido (1..={MAX_SECUENCIA_JS})."
                ),
                recurso_id: None,
            });
        }
        if secuencia <= self.ultima_secuencia {
            return Err(AppError {
                codigo: CodigoError::EntradaInvalida,
                mensaje: format!(
                    "La secuencia ({secuencia}) debe ser estrictamente mayor que la última secuencia registrada ({}).",
                    self.ultima_secuencia
                ),
                recurso_id: None,
            });
        }

        let turno = Turno {
            id: formatear_id_turno(secuencia),
            secuencia,
            fecha_hora_registro: formatear_fecha_iso8601(fecha_hora),
            estado: EstadoTurno::EnEspera,
            ventanilla_asignada: None,
        };

        self.ultima_secuencia = secuencia;
        self.turnos.push(turno.clone());
        Ok(turno)
    }
}
