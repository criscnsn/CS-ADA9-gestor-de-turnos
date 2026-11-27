//! Persistencia SQLite de diferencias calculadas por el dominio.
//!
//! El coordinador debe serializar las operaciones de un único escritor, desde
//! el borrador de dominio hasta su publicación. No se coordinan procesos
//! escritores independientes ni se reservan secuencias en esta capa.

use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use rusqlite::{Connection, Error as SqlError, TransactionBehavior, params};

use crate::{
    db,
    domain::{MAX_SECUENCIA_JS, SistemaTurnos},
    models::{AppError, CodigoError, EstadoTurno, Turno, Ventanilla, VentanillaId},
};

/// Puerto de persistencia independiente del coordinador IPC.
pub trait Repositorio: Send {
    /// Recupera un snapshot consistente y valida sus invariantes estructurales.
    ///
    /// # Errors
    /// Devuelve `BaseDatosCorrupta` para datos incompatibles y `ErrorPersistencia`
    /// para errores de acceso, bloqueo o lectura.
    fn cargar(&mut self) -> Result<SistemaTurnos, AppError>;

    /// Persiste las filas modificadas y el contador en una transacción inmediata.
    ///
    /// # Errors
    /// Devuelve `ErrorPersistencia` cuando no puede confirmar la operación.
    fn guardar(&mut self, cambios: &Cambios) -> Result<(), AppError>;
}

/// Filas nuevas o modificadas por una operación del dominio.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cambios {
    /// Turnos nuevos o modificados; excluir filas terminales sin cambios.
    pub turnos: Vec<Turno>,
    /// Ventanillas modificadas.
    pub ventanillas: Vec<Ventanilla>,
    /// Última secuencia calculada por el dominio.
    pub ultima_secuencia: u64,
}

/// Repositorio que posee una sola conexión SQLite privada.
///
/// Un fallo de rollback o un estado transaccional incierto inutiliza esta
/// instancia; debe cerrarse y abrirse de nuevo. SQLite usa WAL y synchronous
/// NORMAL según la política de [`db`].
///
/// ```
/// use gestor_de_turnos_lib::repository::{Repositorio, RepositorioSqlite};
/// let ruta = std::env::temp_dir().join(format!("sgt-doc-{}-{}.db", std::process::id(),
///     std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
/// let mut repo = RepositorioSqlite::abrir(&ruta).unwrap();
/// let sistema = repo.cargar().unwrap();
/// assert_eq!(sistema.ultima_secuencia(), 0);
/// drop(repo);
/// std::fs::remove_file(ruta).unwrap();
/// ```
pub struct RepositorioSqlite {
    conn: Connection,
    inutilizable: bool,
}

// El temporal se reserva con create_new y comparte directorio con el destino.
// Al cerrar SQLite, la última conexión incorpora el WAL a la base principal.
struct BaseTemporal(PathBuf);

impl Drop for BaseTemporal {
    fn drop(&mut self) {
        for sufijo in ["-wal", "-shm", ""] {
            let mut ruta = self.0.as_os_str().to_os_string();
            ruta.push(sufijo);
            let _ = fs::remove_file(PathBuf::from(ruta));
        }
    }
}

fn abrir_existente(ruta: &Path) -> Result<Connection, AppError> {
    let conn = Connection::open_with_flags(ruta, rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE)
        .map_err(error_lectura)?;
    db::configurar_pragmas(&conn).map_err(error_lectura)?;
    let modo: String = conn
        .query_row("PRAGMA journal_mode", [], |fila| fila.get(0))
        .map_err(error_lectura)?;
    if modo != "wal" {
        return Err(persistencia(format!(
            "No se pudo activar WAL en la base de datos: {modo}"
        )));
    }
    // El cambio a WAL puede aplazar la apertura real de los auxiliares hasta
    // la primera lectura; confirmar que esa apertura también tiene éxito.
    conn.query_row("SELECT count(*) FROM sqlite_master", [], |fila| {
        fila.get::<_, i64>(0)
    })
    .map_err(error_lectura)?;
    Ok(conn)
}

fn preparar_temporal(ruta: &Path) -> Result<BaseTemporal, AppError> {
    static SIGUIENTE: AtomicU64 = AtomicU64::new(0);
    let nombre = ruta
        .file_name()
        .ok_or_else(|| persistencia("Ruta de base de datos inválida"))?;
    loop {
        let mut nombre_temporal = nombre.to_os_string();
        nombre_temporal.push(format!(
            ".tmp-{}-{}",
            std::process::id(),
            SIGUIENTE.fetch_add(1, Ordering::Relaxed)
        ));
        let temporal = ruta.with_file_name(nombre_temporal);
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporal)
        {
            Ok(archivo) => {
                drop(archivo);
                // Un resto de otro proceso no pertenece a esta reserva. Nunca
                // abrir ni limpiar sus archivos auxiliares.
                let auxiliares_libres =
                    ["-wal", "-shm"]
                        .into_iter()
                        .try_fold(true, |libres, sufijo| {
                            let mut auxiliar = temporal.as_os_str().to_os_string();
                            auxiliar.push(sufijo);
                            match fs::symlink_metadata(PathBuf::from(auxiliar)) {
                                Ok(_) => Ok(false),
                                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                                    Ok(libres)
                                }
                                Err(err) => Err(err),
                            }
                        });
                match auxiliares_libres {
                    Ok(true) => {}
                    resultado => {
                        fs::remove_file(&temporal).map_err(persistencia)?;
                        match resultado {
                            Ok(false) => continue,
                            Err(err) => return Err(persistencia(err)),
                            Ok(true) => unreachable!(),
                        }
                    }
                }
                let temporal = BaseTemporal(temporal);
                let conn = db::abrir_conexion_configurada(&temporal.0).map_err(persistencia)?;
                db::inicializar_esquema(&conn).map_err(persistencia)?;
                // sqlite3_close puede ocultar errores del checkpoint implícito.
                // No publicar hasta integrar explícitamente todo el WAL y
                // abandonar ese modo, dejando la base principal autónoma.
                let (busy, log, checkpointed): (i64, i64, i64) = conn
                    .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |fila| {
                        Ok((fila.get(0)?, fila.get(1)?, fila.get(2)?))
                    })
                    .map_err(persistencia)?;
                if busy != 0 || (log != -1 && log != checkpointed) {
                    return Err(persistencia(format!(
                        "Checkpoint incompleto: busy={busy}, log={log}, checkpointed={checkpointed}"
                    )));
                }
                let modo: String = conn
                    .query_row("PRAGMA journal_mode = DELETE", [], |fila| fila.get(0))
                    .map_err(persistencia)?;
                if modo != "delete" {
                    return Err(persistencia(format!(
                        "No se pudo desactivar WAL para publicar la base temporal: {modo}"
                    )));
                }
                conn.close().map_err(|(_, err)| persistencia(err))?;
                return Ok(temporal);
            }
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(err) => return Err(persistencia(err)),
        }
    }
}

impl RepositorioSqlite {
    /// Abre una ruta en disco. Solo un archivo creado aquí recibe semillas.
    ///
    /// Una base existente nunca se reinicializa ni se elimina ante errores.
    /// La validación física adicional pertenece a la integración de resiliencia.
    ///
    /// # Errors
    /// Devuelve `ErrorPersistencia` si no puede crear, abrir o inicializar la base,
    /// o `BaseDatosCorrupta` si SQLite detecta un archivo corrupto o ajeno.
    pub fn abrir(ruta: impl AsRef<Path>) -> Result<Self, AppError> {
        let ruta = ruta.as_ref();
        if let Some(padre) = ruta.parent().filter(|p| !p.as_os_str().is_empty()) {
            fs::create_dir_all(padre).map_err(persistencia)?;
        }
        let existe = match fs::symlink_metadata(ruta) {
            Ok(_) => true,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => false,
            Err(err) => return Err(persistencia(err)),
        };
        let conn = if existe {
            abrir_existente(ruta)?
        } else {
            let temporal = preparar_temporal(ruta)?;
            match fs::hard_link(&temporal.0, ruta) {
                Ok(()) => match abrir_existente(ruta) {
                    Ok(conn) => conn,
                    Err(err) => {
                        if let Err(limpieza) = fs::remove_file(ruta) {
                            return Err(persistencia(format!(
                                "{err:?}; no se pudo retirar la base recién creada: {limpieza}"
                            )));
                        }
                        return Err(err);
                    }
                },
                Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                    abrir_existente(ruta)?
                }
                Err(err) => return Err(persistencia(err)),
            }
        };
        Ok(Self {
            conn,
            inutilizable: false,
        })
    }

    fn comprobar_conexion(&mut self) -> Result<(), AppError> {
        if self.inutilizable || !self.conn.is_autocommit() {
            self.inutilizable = true;
            return Err(persistencia("conexión con estado transaccional incierto"));
        }
        Ok(())
    }
}

fn error(codigo: CodigoError, detalle: impl std::fmt::Display) -> AppError {
    AppError {
        codigo,
        mensaje: detalle.to_string(),
        recurso_id: None,
    }
}

fn persistencia(detalle: impl std::fmt::Display) -> AppError {
    error(CodigoError::ErrorPersistencia, detalle)
}

fn corrupta(detalle: impl std::fmt::Display) -> AppError {
    error(CodigoError::BaseDatosCorrupta, detalle)
}

fn error_lectura(err: SqlError) -> AppError {
    match &err {
        SqlError::InvalidColumnType(..)
        | SqlError::IntegralValueOutOfRange(..)
        | SqlError::FromSqlConversionFailure(..)
        | SqlError::Utf8Error(..)
        | SqlError::QueryReturnedNoRows => corrupta(err),
        SqlError::SqliteFailure(e, _)
            if matches!(
                e.code,
                rusqlite::ErrorCode::DatabaseCorrupt | rusqlite::ErrorCode::NotADatabase
            ) =>
        {
            corrupta(err)
        }
        SqlError::SqliteFailure(e, Some(mensaje))
            if e.code == rusqlite::ErrorCode::Unknown
                && (mensaje.starts_with("no such table:")
                    || mensaje.starts_with("no such column:")) =>
        {
            corrupta(err)
        }
        _ => persistencia(err),
    }
}

fn id_sql(id: VentanillaId) -> &'static str {
    match id {
        VentanillaId::V01 => "V01",
        VentanillaId::V02 => "V02",
        VentanillaId::V03 => "V03",
    }
}

fn leer_id(id: &str) -> Result<VentanillaId, AppError> {
    match id {
        "V01" => Ok(VentanillaId::V01),
        "V02" => Ok(VentanillaId::V02),
        "V03" => Ok(VentanillaId::V03),
        _ => Err(corrupta(format!("Ventanilla desconocida: {id}"))),
    }
}

fn estado_sql(estado: EstadoTurno) -> &'static str {
    match estado {
        EstadoTurno::EnEspera => "EN_ESPERA",
        EstadoTurno::EnAtencion => "EN_ATENCION",
        EstadoTurno::Atendido => "ATENDIDO",
        EstadoTurno::Cancelado => "CANCELADO",
    }
}

fn leer_estado(estado: &str) -> Result<EstadoTurno, AppError> {
    match estado {
        "EN_ESPERA" => Ok(EstadoTurno::EnEspera),
        "EN_ATENCION" => Ok(EstadoTurno::EnAtencion),
        "ATENDIDO" => Ok(EstadoTurno::Atendido),
        "CANCELADO" => Ok(EstadoTurno::Cancelado),
        _ => Err(corrupta(format!("Estado desconocido: {estado}"))),
    }
}

fn leer_secuencia(valor: i64) -> Result<u64, AppError> {
    let valor = u64::try_from(valor).map_err(corrupta)?;
    if valor > MAX_SECUENCIA_JS {
        return Err(corrupta("Secuencia fuera del rango del dominio"));
    }
    Ok(valor)
}

fn secuencia_sql(valor: u64) -> Result<i64, AppError> {
    if valor > MAX_SECUENCIA_JS {
        return Err(persistencia("Secuencia fuera del rango del dominio"));
    }
    i64::try_from(valor).map_err(persistencia)
}

impl Repositorio for RepositorioSqlite {
    fn cargar(&mut self) -> Result<SistemaTurnos, AppError> {
        self.comprobar_conexion()?;
        let tx = self.conn.transaction().map_err(error_lectura)?;
        let resultado = (|| {
            let ultima: i64 = tx
                .query_row(
                    "SELECT ultimo_valor FROM secuencias WHERE clave = 'turno'",
                    [],
                    |r| r.get(0),
                )
                .map_err(error_lectura)?;
            let ultima = leer_secuencia(ultima)?;
            let mut stmt = tx
                .prepare("SELECT id, habilitada, turno_actual_id FROM ventanillas ORDER BY id")
                .map_err(error_lectura)?;
            let filas = stmt
                .query_map([], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, i64>(1)?,
                        r.get::<_, Option<String>>(2)?,
                    ))
                })
                .map_err(error_lectura)?;
            let mut ventanillas = Vec::new();
            for fila in filas {
                let (id, habilitada, turno_actual_id) = fila.map_err(error_lectura)?;
                let habilitada = match habilitada {
                    0 => false,
                    1 => true,
                    _ => return Err(corrupta("Valor de habilitada inválido")),
                };
                ventanillas.push(Ventanilla {
                    id: leer_id(&id)?,
                    habilitada,
                    turno_actual_id,
                });
            }
            let ventanillas: [Ventanilla; 3] = ventanillas
                .try_into()
                .map_err(|_| corrupta("Cantidad de ventanillas inválida"))?;
            let mut stmt = tx.prepare("SELECT id, secuencia, fecha_hora_registro, estado, ventanilla_asignada FROM turnos ORDER BY secuencia").map_err(error_lectura)?;
            let filas = stmt
                .query_map([], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, i64>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, Option<String>>(4)?,
                    ))
                })
                .map_err(error_lectura)?;
            let mut turnos = Vec::new();
            for fila in filas {
                let (id, secuencia, fecha_hora_registro, estado, ventanilla) =
                    fila.map_err(error_lectura)?;
                turnos.push(Turno {
                    id,
                    secuencia: leer_secuencia(secuencia)?,
                    fecha_hora_registro,
                    estado: leer_estado(&estado)?,
                    ventanilla_asignada: ventanilla.as_deref().map(leer_id).transpose()?,
                });
            }
            SistemaTurnos::restaurar(ventanillas, turnos, ultima)
        })();
        let resultado = match resultado {
            Ok(sistema) => tx.commit().map(|()| sistema).map_err(error_lectura),
            Err(err) => match tx.rollback() {
                Ok(()) => Err(err),
                Err(rollback) => {
                    self.inutilizable = true;
                    Err(persistencia(format!("{err:?}; rollback: {rollback}")))
                }
            },
        };
        if !self.conn.is_autocommit() {
            self.inutilizable = true;
            return Err(persistencia("Estado transaccional incierto tras cargar"));
        }
        resultado
    }

    fn guardar(&mut self, cambios: &Cambios) -> Result<(), AppError> {
        self.comprobar_conexion()?;
        let ultima = secuencia_sql(cambios.ultima_secuencia)?;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(persistencia)?;
        let resultado = (|| {
            let anterior: i64 = tx
                .query_row(
                    "SELECT ultimo_valor FROM secuencias WHERE clave = 'turno'",
                    [],
                    |r| r.get(0),
                )
                .map_err(persistencia)?;
            if anterior < 0 || anterior > ultima {
                return Err(persistencia("Secuencia persistida inválida o decreciente"));
            }
            for turno in cambios
                .turnos
                .iter()
                .filter(|t| t.estado != EstadoTurno::EnAtencion)
                .chain(
                    cambios
                        .turnos
                        .iter()
                        .filter(|t| t.estado == EstadoTurno::EnAtencion),
                )
            {
                let secuencia = secuencia_sql(turno.secuencia)?;
                if secuencia == 0 || secuencia > ultima {
                    return Err(persistencia("Secuencia de turno inválida"));
                }
                tx.execute("INSERT INTO turnos (id, secuencia, fecha_hora_registro, estado, ventanilla_asignada) VALUES (?1, ?2, ?3, ?4, ?5) ON CONFLICT(id) DO UPDATE SET secuencia=excluded.secuencia, fecha_hora_registro=excluded.fecha_hora_registro, estado=excluded.estado, ventanilla_asignada=excluded.ventanilla_asignada", params![turno.id, secuencia, turno.fecha_hora_registro, estado_sql(turno.estado), turno.ventanilla_asignada.map(id_sql)]).map_err(persistencia)?;
            }
            for habilitada in [true, false] {
                for ventanilla in cambios
                    .ventanillas
                    .iter()
                    .filter(|v| v.habilitada == habilitada)
                {
                    let count = tx
                        .execute(
                            "UPDATE ventanillas SET habilitada=?1, turno_actual_id=?2 WHERE id=?3",
                            params![
                                ventanilla.habilitada,
                                ventanilla.turno_actual_id,
                                id_sql(ventanilla.id)
                            ],
                        )
                        .map_err(persistencia)?;
                    if count != 1 {
                        return Err(persistencia("No existe la ventanilla modificada"));
                    }
                }
            }
            let count = tx
                .execute(
                    "UPDATE secuencias SET ultimo_valor=?1 WHERE clave='turno'",
                    [ultima],
                )
                .map_err(persistencia)?;
            if count != 1 {
                return Err(persistencia("No existe la secuencia de turnos"));
            }
            Ok(())
        })();
        let resultado = match resultado {
            Ok(()) => tx.commit().map_err(persistencia),
            Err(err) => match tx.rollback() {
                Ok(()) => Err(err),
                Err(rollback) => {
                    self.inutilizable = true;
                    Err(persistencia(format!("{err:?}; rollback: {rollback}")))
                }
            },
        };
        if !self.conn.is_autocommit() {
            self.inutilizable = true;
            return Err(persistencia("Estado transaccional incierto tras guardar"));
        }
        resultado
    }
}
