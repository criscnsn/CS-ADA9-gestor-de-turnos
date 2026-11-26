//! Dominio puro del Sistema de Gestión de Turnos (SGT).
//!
//! Este módulo es dueño del estado en memoria [`SistemaTurnos`] y separa las
//! responsabilidades en submódulos independientes:
//! - [`registro`]: creación de turnos en [`EstadoTurno::EnEspera`] con
//!   secuencia monotónica creciente (`RN-01`, `RN-02`, `RF-02`).
//! - [`consultas`]: consultas de ventanillas (`RF-01`), fila FIFO (`RF-03`),
//!   historial completo (`RF-10`) y snapshots [`EstadoSistema`].
//! - [`transiciones`]: asignación FIFO (`RF-04`, `RF-05`), finalización
//!   (`RF-06`, `RF-07`) y cancelación (`RF-08`, `RF-09`), incluyendo el guard
//!   de ventanilla habilitada (`RF-13`).
//!
//! La habilitación/inhabilitación de ventanillas (`#10`, `RF-13`) se incorpora
//! como submódulo hermano usando las firmas de acceso al estado expuestas por
//! [`SistemaTurnos`], sin acoplar este núcleo a Tauri ni a SQLite (`RNF-M01`).
//!
//! # Coordinación de secuencia con persistencia (`#12`)
//!
//! [`SistemaTurnos`] no mantiene un contador global independiente: su
//! [`SistemaTurnos::ultima_secuencia`] se restaura desde el repositorio mediante
//! [`SistemaTurnos::restaurar`] o [`SistemaTurnos::desde_snapshot`], y admite
//! registrar turnos con la secuencia producida por la transacción SQLite a
//! través de [`SistemaTurnos::registrar_turno_con_secuencia`].

pub mod consultas;
pub mod registro;
pub mod transiciones;

use std::collections::HashSet;

use crate::models::{
    AppError, CodigoError, EstadoSistema, EstadoTurno, Turno, Ventanilla, VentanillaId,
};

pub use registro::{MAX_SECUENCIA_JS, formatear_fecha_iso8601, formatear_id_turno};

/// Orden canónico de las tres ventanillas predefinidas (`RN-03`).
pub const VENTANILLAS_ORDEN: [VentanillaId; 3] =
    [VentanillaId::V01, VentanillaId::V02, VentanillaId::V03];

/// Estado de dominio en memoria del Sistema de Gestión de Turnos (SGT).
///
/// Es la fuente única de verdad para el registro y las consultas puras del
/// dominio. Deriva [`Clone`] para permitir operaciones atómicas tipo borrador
/// antes de confirmar cambios en persistencia (`RD-02`, `RNF-R02`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SistemaTurnos {
    ventanillas: [Ventanilla; 3],
    turnos: Vec<Turno>,
    ultima_secuencia: u64,
}

impl Default for SistemaTurnos {
    fn default() -> Self {
        Self::nuevo()
    }
}

impl SistemaTurnos {
    /// Crea un sistema vacío con las tres ventanillas (`V01`, `V02`, `V03`)
    /// habilitadas y libres, sin turnos registrados y con secuencia inicial `0`.
    #[must_use]
    pub fn nuevo() -> Self {
        Self {
            ventanillas: VENTANILLAS_ORDEN.map(|id| Ventanilla {
                id,
                habilitada: true,
                turno_actual_id: None,
            }),
            turnos: Vec::new(),
            ultima_secuencia: 0,
        }
    }

    /// Restaura el estado de dominio a partir de los datos recuperados desde el
    /// repositorio (`#12`), preservando el contador persistente `ultima_secuencia`
    /// para evitar reutilización de identificadores tras reinicios (`CV-08`).
    ///
    /// Normaliza las ventanillas en orden canónico (`V01`, `V02`, `V03`) y
    /// valida que no existan ventanillas duplicadas, secuencias inválidas o
    /// duplicadas, identificadores repetidos ni turnos cuya secuencia supere
    /// `ultima_secuencia`.
    ///
    /// # Errors
    ///
    /// Devuelve [`CodigoError::BaseDatosCorrupta`] si los datos restaurados
    /// violan los invariantes estructurales del dominio.
    pub fn restaurar(
        ventanillas: [Ventanilla; 3],
        historial: Vec<Turno>,
        ultima_secuencia: u64,
    ) -> Result<Self, AppError> {
        if ultima_secuencia > MAX_SECUENCIA_JS {
            return Err(AppError {
                codigo: CodigoError::BaseDatosCorrupta,
                mensaje: format!(
                    "La secuencia persistida ({ultima_secuencia}) excede el rango seguro admitido."
                ),
                recurso_id: None,
            });
        }

        let ventanillas_ordenadas = ordenar_y_validar_ventanillas(ventanillas)?;

        if !ventanillas_ordenadas.iter().any(|v| v.habilitada) {
            return Err(AppError {
                codigo: CodigoError::BaseDatosCorrupta,
                mensaje: "No hay ninguna ventanilla habilitada al restaurar el estado.".to_string(),
                recurso_id: None,
            });
        }

        let mut turnos_en_ventanillas = HashSet::with_capacity(3);
        for ventanilla in &ventanillas_ordenadas {
            if let Some(turno_id) = &ventanilla.turno_actual_id {
                if !ventanilla.habilitada {
                    return Err(AppError {
                        codigo: CodigoError::BaseDatosCorrupta,
                        mensaje: format!(
                            "La ventanilla {:?} está inhabilitada pero tiene asignado el turno {turno_id}.",
                            ventanilla.id
                        ),
                        recurso_id: Some(format!("{:?}", ventanilla.id)),
                    });
                }
                if !turnos_en_ventanillas.insert(turno_id.as_str()) {
                    return Err(AppError {
                        codigo: CodigoError::BaseDatosCorrupta,
                        mensaje: format!(
                            "El turno {turno_id} figura asignado a más de una ventanilla."
                        ),
                        recurso_id: Some(turno_id.clone()),
                    });
                }
            }
        }

        let mut ids_vistos = HashSet::with_capacity(historial.len());
        let mut secuencias_vistas = HashSet::with_capacity(historial.len());
        let mut turnos_en_atencion_validados = 0usize;

        for turno in &historial {
            if turno.id.trim().is_empty() {
                return Err(AppError {
                    codigo: CodigoError::BaseDatosCorrupta,
                    mensaje: "Se encontró un turno con identificador vacío al restaurar el estado."
                        .to_string(),
                    recurso_id: None,
                });
            }
            if turno.fecha_hora_registro.trim().is_empty() {
                return Err(AppError {
                    codigo: CodigoError::BaseDatosCorrupta,
                    mensaje: format!(
                        "El turno {} no tiene fecha de registro al restaurar el estado.",
                        turno.id
                    ),
                    recurso_id: Some(turno.id.clone()),
                });
            }
            if turno.secuencia == 0 || turno.secuencia > ultima_secuencia {
                return Err(AppError {
                    codigo: CodigoError::BaseDatosCorrupta,
                    mensaje: format!(
                        "El turno {} tiene una secuencia inválida ({}) respecto a la última secuencia restaurada ({ultima_secuencia}).",
                        turno.id, turno.secuencia
                    ),
                    recurso_id: Some(turno.id.clone()),
                });
            }
            if !ids_vistos.insert(turno.id.as_str()) {
                return Err(AppError {
                    codigo: CodigoError::BaseDatosCorrupta,
                    mensaje: format!(
                        "Se encontró un identificador de turno duplicado ({}) al restaurar el estado.",
                        turno.id
                    ),
                    recurso_id: Some(turno.id.clone()),
                });
            }
            if !secuencias_vistas.insert(turno.secuencia) {
                return Err(AppError {
                    codigo: CodigoError::BaseDatosCorrupta,
                    mensaje: format!(
                        "Se encontró una secuencia duplicada ({}) en el turno {}.",
                        turno.secuencia, turno.id
                    ),
                    recurso_id: Some(turno.id.clone()),
                });
            }

            match (turno.estado, turno.ventanilla_asignada) {
                (EstadoTurno::EnEspera | EstadoTurno::Cancelado, Some(vid)) => {
                    return Err(AppError {
                        codigo: CodigoError::BaseDatosCorrupta,
                        mensaje: format!(
                            "El turno {} en estado {:?} no puede tener ventanilla asignada ({vid:?}).",
                            turno.id, turno.estado
                        ),
                        recurso_id: Some(turno.id.clone()),
                    });
                }
                (EstadoTurno::EnAtencion | EstadoTurno::Atendido, None) => {
                    return Err(AppError {
                        codigo: CodigoError::BaseDatosCorrupta,
                        mensaje: format!(
                            "El turno {} en estado {:?} debe tener una ventanilla asignada.",
                            turno.id, turno.estado
                        ),
                        recurso_id: Some(turno.id.clone()),
                    });
                }
                (EstadoTurno::EnAtencion, Some(vid)) => {
                    let ventanilla = &ventanillas_ordenadas[Self::indice_ventanilla(vid)];
                    if ventanilla.turno_actual_id.as_deref() != Some(turno.id.as_str()) {
                        return Err(AppError {
                            codigo: CodigoError::BaseDatosCorrupta,
                            mensaje: format!(
                                "El turno {} figura en atención en {vid:?}, pero la ventanilla no coincide.",
                                turno.id
                            ),
                            recurso_id: Some(turno.id.clone()),
                        });
                    }
                    turnos_en_atencion_validados += 1;
                }
                _ => {}
            }
        }

        if turnos_en_atencion_validados != turnos_en_ventanillas.len() {
            return Err(AppError {
                codigo: CodigoError::BaseDatosCorrupta,
                mensaje:
                    "Una o más ventanillas referencian un turno actual inexistente o no en atención."
                        .to_string(),
                recurso_id: None,
            });
        }

        Ok(Self {
            ventanillas: ventanillas_ordenadas,
            turnos: historial,
            ultima_secuencia,
        })
    }

    /// Construye el estado de dominio a partir de un [`EstadoSistema`] (por
    /// ejemplo, fixtures compartidos del contrato o snapshots de pruebas).
    ///
    /// Si `ultima_secuencia` es [`None`], se deriva como la mayor
    /// [`Turno::secuencia`] presente en `snapshot.historial` (o `0` si está
    /// vacío). Si es [`Some`], se conserva exactamente el valor indicado por el
    /// repositorio.
    ///
    /// # Errors
    ///
    /// Devuelve [`CodigoError::BaseDatosCorrupta`] si el snapshot no cumple los
    /// invariantes estructurales verificados por [`Self::restaurar`].
    pub fn desde_snapshot(
        snapshot: EstadoSistema,
        ultima_secuencia: Option<u64>,
    ) -> Result<Self, AppError> {
        let max_historial = snapshot
            .historial
            .iter()
            .map(|turno| turno.secuencia)
            .max()
            .unwrap_or(0);
        let secuencia_efectiva = ultima_secuencia.unwrap_or(max_historial);
        Self::restaurar(snapshot.ventanillas, snapshot.historial, secuencia_efectiva)
    }

    /// Devuelve la última secuencia asignada (o `0` si todavía no existen turnos).
    #[must_use]
    pub fn ultima_secuencia(&self) -> u64 {
        self.ultima_secuencia
    }

    /// Devuelve el índice canónico (`0`, `1` o `2`) de una [`VentanillaId`].
    #[must_use]
    pub(crate) fn indice_ventanilla(id: VentanillaId) -> usize {
        match id {
            VentanillaId::V01 => 0,
            VentanillaId::V02 => 1,
            VentanillaId::V03 => 2,
        }
    }

    /// Acceso inmutable a la colección interna de turnos para módulos hermanos
    /// del dominio (`#9`, `#10`).
    #[must_use]
    #[allow(dead_code)]
    pub(crate) fn turnos_slice(&self) -> &[Turno] {
        &self.turnos
    }

    /// Acceso mutable a las tres ventanillas para módulos hermanos (`#9`, `#10`).
    #[allow(dead_code)]
    pub(crate) fn ventanillas_mut(&mut self) -> &mut [Ventanilla; 3] {
        &mut self.ventanillas
    }

    /// Acceso mutable a una ventanilla específica por [`VentanillaId`] (`#9`, `#10`).
    #[allow(dead_code)]
    pub(crate) fn ventanilla_mut(&mut self, id: VentanillaId) -> &mut Ventanilla {
        let indice = Self::indice_ventanilla(id);
        &mut self.ventanillas[indice]
    }

    /// Acceso mutable al vector interno de turnos para transiciones (`#9`).
    #[allow(dead_code)]
    pub(crate) fn turnos_mut(&mut self) -> &mut Vec<Turno> {
        &mut self.turnos
    }

    /// Busca un turno mutable por su identificador exacto para transiciones (`#9`).
    #[allow(dead_code)]
    pub(crate) fn buscar_turno_mut(&mut self, turno_id: &str) -> Option<&mut Turno> {
        self.turnos.iter_mut().find(|turno| turno.id == turno_id)
    }

    /// Devuelve una referencia mutable al turno en [`EstadoTurno::EnEspera`] con
    /// menor [`Turno::secuencia`] para la asignación FIFO (`#9`, `RF-04`, `RN-02`).
    #[allow(dead_code)]
    pub(crate) fn siguiente_en_espera_mut(&mut self) -> Option<&mut Turno> {
        self.turnos
            .iter_mut()
            .filter(|turno| turno.estado == EstadoTurno::EnEspera)
            .min_by_key(|turno| turno.secuencia)
    }
}

fn ordenar_y_validar_ventanillas(
    ventanillas: [Ventanilla; 3],
) -> Result<[Ventanilla; 3], AppError> {
    let mut v01 = None;
    let mut v02 = None;
    let mut v03 = None;

    for ventanilla in ventanillas {
        let destino = match ventanilla.id {
            VentanillaId::V01 => &mut v01,
            VentanillaId::V02 => &mut v02,
            VentanillaId::V03 => &mut v03,
        };
        if destino.is_some() {
            return Err(AppError {
                codigo: CodigoError::BaseDatosCorrupta,
                mensaje: format!(
                    "Se encontró la ventanilla {:?} duplicada al restaurar el estado.",
                    ventanilla.id
                ),
                recurso_id: Some(format!("{:?}", ventanilla.id)),
            });
        }
        *destino = Some(ventanilla);
    }

    match (v01, v02, v03) {
        (Some(v01), Some(v02), Some(v03)) => Ok([v01, v02, v03]),
        _ => Err(AppError {
            codigo: CodigoError::BaseDatosCorrupta,
            mensaje: "Faltan ventanillas predefinidas (V01, V02, V03) al restaurar el estado."
                .to_string(),
            recurso_id: None,
        }),
    }
}
