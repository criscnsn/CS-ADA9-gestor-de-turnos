//! Tipos de datos que comparten el backend SGT y el frontend mediante JSON.
//!
//! [`Turno`], [`Ventanilla`] y [`EstadoSistema`] describen los datos del sistema;
//! [`AppError`] representa los rechazos de comandos. Este módulo solo define el
//! formato de intercambio: no ejecuta comandos Tauri, consultas SQL ni reglas de
//! negocio.
//!
//! # Formato de intercambio
//!
//! - Los campos de las estructuras usan `camelCase` en JSON.
//! - [`EstadoTurno`] y [`CodigoError`] conservan valores `SCREAMING_SNAKE_CASE`.
//! - [`VentanillaId`] conserva `V01`, `V02` y `V03`; [`TipoVista`] usa minúsculas.
//! - Los campos opcionales se serializan como `null` cuando contienen [`None`].
//!
//! Los ejemplos JSON comunes se encuentran en `docs/fixtures/contrato/` y su
//! especificación en `docs/contrato_base.md`, relativos a la raíz del repositorio.
//!
//! # Validación
//!
//! Serde rechaza variantes desconocidas y snapshots con una cantidad de
//! ventanillas distinta de tres. Deserializar estos tipos no verifica el formato
//! de fechas, la unicidad de identificadores, la consistencia entre turnos y
//! ventanillas ni las transiciones permitidas: esas reglas pertenecen al dominio.
//!
//! # Examples
//!
//! ```
//! use gestor_de_turnos_lib::models::{EstadoTurno, TipoVista};
//!
//! assert_eq!(serde_json::to_string(&EstadoTurno::EnEspera)?, r#""EN_ESPERA""#);
//! assert_eq!(serde_json::to_string(&TipoVista::Cliente)?, r#""cliente""#);
//! # Ok::<(), serde_json::Error>(())
//! ```

use serde::{Deserialize, Serialize};

/// Estado de un turno dentro de su ciclo de atención.
///
/// Los valores JSON son estables y no dependen de las etiquetas visibles de la
/// interfaz. Las operaciones del dominio determinan las transiciones permitidas.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum EstadoTurno {
    /// Turno pendiente en la fila FIFO. JSON: `"EN_ESPERA"`.
    EnEspera,
    /// Turno que está siendo atendido en una ventanilla. JSON: `"EN_ATENCION"`.
    EnAtencion,
    /// Atención finalizada; el turno se conserva en el historial. JSON: `"ATENDIDO"`.
    Atendido,
    /// Turno retirado de la fila sin eliminar su historial. JSON: `"CANCELADO"`.
    Cancelado,
}

/// Identificador de una de las tres ventanillas fijas del sistema.
///
/// Su representación JSON coincide con el nombre de cada variante. Habilitar o
/// inhabilitar una ventanilla no modifica su identificador.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VentanillaId {
    /// Primera ventanilla. JSON: `"V01"`.
    V01,
    /// Segunda ventanilla. JSON: `"V02"`.
    V02,
    /// Tercera ventanilla. JSON: `"V03"`.
    V03,
}

/// Vista solicitada al gestor de ventanas mediante el comando `abrir_ventana`.
///
/// Este tipo solo describe el argumento; no crea ni enfoca ventanas por sí mismo.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TipoVista {
    /// Interfaz de solicitud y seguimiento de turnos. JSON: `"cliente"`.
    Cliente,
    /// Interfaz de operación de las ventanillas. JSON: `"cajero"`.
    Cajero,
}

/// Datos de un turno registrado, conservados durante todo su ciclo de atención.
///
/// El dominio crea el turno en [`EstadoTurno::EnEspera`] y aplica las reglas de
/// asignación, cancelación y finalización. Este contenedor no valida esas reglas.
///
/// # Examples
///
/// ```
/// use gestor_de_turnos_lib::models::{EstadoTurno, Turno};
///
/// let turno = Turno {
///     id: "T-0001".into(),
///     secuencia: 1,
///     fecha_hora_registro: "2026-11-18T01:00:00Z".into(),
///     estado: EstadoTurno::EnEspera,
///     ventanilla_asignada: None,
/// };
/// let json = serde_json::to_value(&turno)?;
/// assert_eq!(json["fechaHoraRegistro"], "2026-11-18T01:00:00Z");
/// assert!(json["ventanillaAsignada"].is_null());
/// assert_eq!(serde_json::from_value::<Turno>(json)?, turno);
/// # Ok::<(), serde_json::Error>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Turno {
    /// Identificador legible único, por ejemplo `T-0001`; no debe reutilizarse.
    pub id: String,
    /// Orden monotónico de registro que determina la posición FIFO.
    ///
    /// El dominio y la persistencia deben mantenerlo en un rango entero
    /// representable exactamente por el `number` de TypeScript.
    pub secuencia: u64,
    /// Instante de registro expresado como texto ISO-8601.
    ///
    /// JSON: `fechaHoraRegistro`. Serde no valida el contenido de la fecha.
    pub fecha_hora_registro: String,
    /// Estado actual del turno dentro de su ciclo de atención.
    pub estado: EstadoTurno,
    /// Ventanilla que atiende o atendió el turno, si existe.
    ///
    /// JSON: `ventanillaAsignada`. Es `null` mientras espera o está cancelado
    /// sin atención; el dominio exige un valor en atención y tras finalizar.
    pub ventanilla_asignada: Option<VentanillaId>,
}

/// Estado de una ventanilla, independiente de su representación visual.
///
/// Su estado visible se deriva de [`Self::habilitada`] y
/// [`Self::turno_actual_id`]. El dominio impide inhabilitar una ventanilla ocupada
/// y exige mantener al menos una habilitada.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Ventanilla {
    /// Identificador fijo de la ventanilla.
    pub id: VentanillaId,
    /// Indica si puede recibir una nueva asignación cuando está libre.
    pub habilitada: bool,
    /// ID del turno en [`EstadoTurno::EnAtencion`], o [`None`] cuando está libre.
    ///
    /// JSON: `turnoActualId`; [`None`] se serializa como `null`.
    pub turno_actual_id: Option<String>,
}

/// Snapshot completo destinado a consultas y notificaciones de estado.
///
/// Es el payload definido para `obtener_estado` y `sgt://estado-actualizado`.
/// Este tipo no implementa la emisión del evento ni la lectura de persistencia.
///
/// # Examples
///
/// ```
/// use gestor_de_turnos_lib::models::{EstadoSistema, Ventanilla, VentanillaId};
///
/// let estado = EstadoSistema {
///     ventanillas: [VentanillaId::V01, VentanillaId::V02, VentanillaId::V03]
///         .map(|id| Ventanilla { id, habilitada: true, turno_actual_id: None }),
///     fila_espera: vec![],
///     historial: vec![],
/// };
/// let json = serde_json::to_value(&estado)?;
/// assert_eq!(json["ventanillas"].as_array().unwrap().len(), 3);
/// assert_eq!(json["filaEspera"], serde_json::json!([]));
/// # Ok::<(), serde_json::Error>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EstadoSistema {
    /// Las tres ventanillas fijas; el dominio garantiza sus IDs únicos y válidos.
    pub ventanillas: [Ventanilla; 3],
    /// Turnos en espera ordenados de menor a mayor [`Turno::secuencia`].
    ///
    /// JSON: `filaEspera`. El dominio mantiene el filtro y el orden de la fila.
    pub fila_espera: Vec<Turno>,
    /// Todos los turnos registrados, incluidos los atendidos y cancelados.
    pub historial: Vec<Turno>,
}

/// Clasificación estable de los rechazos que puede recibir el frontend.
///
/// El consumidor debe distinguir errores por este código, no por el texto de
/// [`AppError::mensaje`]. Los valores JSON usan `SCREAMING_SNAKE_CASE`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CodigoError {
    /// El identificador no corresponde a una ventanilla del sistema.
    VentanillaInvalida,
    /// La operación requiere una ventanilla libre, pero está atendiendo un turno.
    VentanillaOcupada,
    /// Se intenta finalizar atención en una ventanilla que no tiene turno actual.
    VentanillaLibre,
    /// Se intenta asignar un turno a una ventanilla deshabilitada.
    VentanillaInhabilitada,
    /// La inhabilitación dejaría al sistema sin ventanillas habilitadas.
    UltimaVentanillaActiva,
    /// No hay turnos en espera para una nueva asignación.
    FilaVacia,
    /// No existe un turno con el identificador solicitado.
    TurnoInexistente,
    /// El estado actual del turno no permite la operación solicitada.
    EstadoTurnoInvalido,
    /// Un argumento no cumple el formato o los valores admitidos.
    EntradaInvalida,
    /// La operación de persistencia no pudo completarse correctamente.
    ErrorPersistencia,
    /// La base de datos es ilegible o no supera las comprobaciones de integridad.
    BaseDatosCorrupta,
}

/// Rechazo estructurado de un comando, serializable sin perder su código.
///
/// El backend aporta un mensaje en español y el recurso implicado cuando aplica.
/// El consumidor identifica la causa mediante [`Self::codigo`].
///
/// # Examples
///
/// ```
/// use gestor_de_turnos_lib::models::{AppError, CodigoError};
///
/// let error = AppError {
///     codigo: CodigoError::TurnoInexistente,
///     mensaje: "El turno T-0042 no existe.".into(),
///     recurso_id: Some("T-0042".into()),
/// };
/// let json = serde_json::to_value(&error)?;
/// assert_eq!(json["codigo"], "TURNO_INEXISTENTE");
/// assert_eq!(json["recursoId"], "T-0042");
/// # Ok::<(), serde_json::Error>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppError {
    /// Código estable que describe la causa del rechazo.
    pub codigo: CodigoError,
    /// Mensaje explicativo en español, incluyendo el ID implicado cuando aplica.
    pub mensaje: String,
    /// ID del turno o ventanilla implicado, o [`None`] si no hay un recurso concreto.
    ///
    /// JSON: `recursoId`; [`None`] se serializa como `null`.
    pub recurso_id: Option<String>,
}
