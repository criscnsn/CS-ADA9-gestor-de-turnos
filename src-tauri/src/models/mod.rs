//! Payloads JSON del contrato SGT. Las reglas de negocio pertenecen al dominio.

use serde::{Deserialize, Serialize};

/// Estado del turno; sus valores JSON son parte del contrato público.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum EstadoTurno {
    EnEspera,
    EnAtencion,
    Atendido,
    Cancelado,
}

/// Las tres ventanillas fijas del sistema.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VentanillaId {
    V01,
    V02,
    V03,
}

/// Vistas admitidas por el comando abrir_ventana.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TipoVista {
    Cliente,
    Cajero,
}

/// Turno registrado. La secuencia determina el orden FIFO.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Turno {
    pub id: String,
    pub secuencia: u64,
    /// Fecha/hora ISO-8601; el dominio valida y genera su contenido.
    pub fecha_hora_registro: String,
    pub estado: EstadoTurno,
    pub ventanilla_asignada: Option<VentanillaId>,
}

/// Estado persistible de una ventanilla.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Ventanilla {
    pub id: VentanillaId,
    pub habilitada: bool,
    pub turno_actual_id: Option<String>,
}

/// Snapshot completo enviado por obtener_estado y sgt://estado-actualizado.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EstadoSistema {
    pub ventanillas: [Ventanilla; 3],
    pub fila_espera: Vec<Turno>,
    pub historial: Vec<Turno>,
}

/// Códigos estables compartidos con el frontend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CodigoError {
    VentanillaInvalida,
    VentanillaOcupada,
    VentanillaLibre,
    VentanillaInhabilitada,
    UltimaVentanillaActiva,
    FilaVacia,
    TurnoInexistente,
    EstadoTurnoInvalido,
    EntradaInvalida,
    ErrorPersistencia,
    BaseDatosCorrupta,
}

/// Rechazo de un comando: mensaje en español y recurso implicado cuando aplica.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppError {
    pub codigo: CodigoError,
    pub mensaje: String,
    pub recurso_id: Option<String>,
}
