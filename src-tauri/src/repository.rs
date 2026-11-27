//! Persistencia SQLite de diferencias calculadas por el dominio.
//!
//! El coordinador debe serializar las operaciones de un único escritor, desde
//! el borrador de dominio hasta su publicación. No se coordinan procesos
//! escritores independientes ni se reservan secuencias en esta capa.

use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use rusqlite::{Connection, Error as SqlError, TransactionBehavior, config::DbConfig, params};

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

    /// Programa un único fallo controlado en la próxima escritura para pruebas (`CV-09`).
    #[cfg(feature = "test-fault-injection")]
    fn inyectar_fallo_escritura(&mut self, _punto: PuntoFalloEscritura) {}
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
    wal_pendiente: bool,
    checkpoint_protegido: bool,
    sonda: Option<DirectorioSonda>,
    ruta_sonda: Option<PathBuf>,
    #[cfg(feature = "test-fault-injection")]
    fallo_escritura: Option<PuntoFalloEscritura>,
}

/// Puntos de fallo de un solo disparo para probar rollback de escrituras.
#[cfg(feature = "test-fault-injection")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PuntoFalloEscritura {
    /// Después de escribir los turnos.
    DespuesDeTurnos,
    /// Después de escribir las ventanillas.
    DespuesDeVentanillas,
    /// Después de actualizar la secuencia y antes del commit.
    AntesDeCommit,
}

#[cfg(feature = "test-fault-injection")]
fn comprobar_fallo(
    marcador: &mut Option<PuntoFalloEscritura>,
    punto: PuntoFalloEscritura,
) -> Result<(), AppError> {
    if *marcador == Some(punto) {
        marcador.take();
        return Err(persistencia(format!(
            "Fallo de escritura inyectado: {punto:?}"
        )));
    }
    Ok(())
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

fn auxiliar(ruta: &Path, sufijo: &str) -> PathBuf {
    let mut nombre = ruta.as_os_str().to_os_string();
    nombre.push(sufijo);
    PathBuf::from(nombre)
}

fn palabra_be(bytes: &[u8]) -> u32 {
    u32::from_be_bytes(bytes.try_into().expect("palabra de cuatro bytes"))
}

// SQLite define una suma acumulativa de pares de palabras de 32 bits.
fn checksum_wal(datos: &[u8], little: bool, suma: &mut (u32, u32)) {
    for par in datos.as_chunks::<8>().0 {
        let palabra = |b: &[u8]| {
            let bytes = b.try_into().expect("palabra de cuatro bytes");
            if little {
                u32::from_le_bytes(bytes)
            } else {
                u32::from_be_bytes(bytes)
            }
        };
        suma.0 = suma.0.wrapping_add(palabra(&par[..4])).wrapping_add(suma.1);
        suma.1 = suma.1.wrapping_add(palabra(&par[4..])).wrapping_add(suma.0);
    }
}

fn validar_wal(ruta: &Path, tam_pagina: usize) -> Result<(), AppError> {
    let mut archivo = fs::File::open(ruta).map_err(persistencia)?;
    let len = archivo.metadata().map_err(persistencia)?.len();
    if len == 0 {
        return Ok(());
    }
    let frame_len = 24_u64 + tam_pagina as u64;
    if len < 32 {
        return Err(corrupta("Cabecera WAL truncada"));
    }
    let mut cabecera = [0_u8; 32];
    archivo.read_exact(&mut cabecera).map_err(persistencia)?;
    let little = match palabra_be(&cabecera[..4]) {
        0x377f_0682 => true,
        0x377f_0683 => false,
        _ => return Err(corrupta("Magia WAL inválida")),
    };
    if palabra_be(&cabecera[4..8]) != 3_007_000 || palabra_be(&cabecera[8..12]) != tam_pagina as u32
    {
        return Err(corrupta("Versión o tamaño de página WAL inválido"));
    }
    let mut suma = (0_u32, 0_u32);
    checksum_wal(&cabecera[..24], little, &mut suma);
    if suma != (palabra_be(&cabecera[24..28]), palabra_be(&cabecera[28..32])) {
        return Err(corrupta("Checksum de cabecera WAL inválido"));
    }
    let checkpoint = palabra_be(&cabecera[12..16]);
    let sal = &cabecera[16..24];
    let mut reciclado = false;
    let mut en_cadena_activa = true;
    let mut frame = vec![0_u8; 24 + tam_pagina];
    for _ in 0..((len - 32) / frame_len) {
        archivo.read_exact(&mut frame).map_err(persistencia)?;
        if palabra_be(&frame[..4]) == 0 {
            return Err(corrupta("Número de página WAL inválido"));
        }
        let tam_commit = palabra_be(&frame[4..8]);
        let almacenada = (palabra_be(&frame[16..20]), palabra_be(&frame[20..24]));
        if &frame[8..16] != sal {
            if en_cadena_activa {
                let mut hipotetica = suma;
                checksum_wal(&frame[..8], little, &mut hipotetica);
                checksum_wal(&frame[24..], little, &mut hipotetica);
                if hipotetica == almacenada {
                    return Err(corrupta("Salt de frame vigente WAL alterado"));
                }
            }
            en_cadena_activa = false;
            // SQLite puede reservar frames para un spill aún no finalizado.
            if frame[8..16] == [0; 8] {
                if tam_commit != 0 || almacenada != (0, 0) {
                    return Err(corrupta("Slot de spill WAL inválido"));
                }
                continue;
            }
            let distancia = palabra_be(&cabecera[16..20]).wrapping_sub(palabra_be(&frame[8..12]));
            if checkpoint == 0
                || distancia == 0
                || distancia > checkpoint
                || frame[12..16] == cabecera[20..24]
            {
                return Err(corrupta("Salt WAL inválido"));
            }
            reciclado = true;
            continue;
        }
        if reciclado {
            return Err(corrupta("Frame WAL vigente después de cola reciclada"));
        }
        if en_cadena_activa {
            let mut siguiente = suma;
            checksum_wal(&frame[..8], little, &mut siguiente);
            checksum_wal(&frame[24..], little, &mut siguiente);
            if siguiente == almacenada {
                suma = siguiente;
            } else if tam_commit != 0 {
                return Err(corrupta("Checksum de commit WAL inválido"));
            } else {
                // Tras rollback SQLite puede dejar páginas de spill obsoletas.
                en_cadena_activa = false;
            }
        } else if tam_commit != 0 {
            return Err(corrupta("Commit WAL después de frame inválido"));
        }
    }
    if reciclado && (len - 32) % frame_len != 0 {
        return Err(corrupta("Cola WAL reciclada truncada"));
    }
    Ok(())
}

struct DirectorioSonda(PathBuf);

impl DirectorioSonda {
    fn crear() -> Result<Self, AppError> {
        static SIGUIENTE: AtomicU64 = AtomicU64::new(0);
        loop {
            let ruta = std::env::temp_dir().join(format!(
                "sgt-sonda-{}-{}",
                std::process::id(),
                SIGUIENTE.fetch_add(1, Ordering::Relaxed)
            ));
            let builder = fs::DirBuilder::new();
            #[cfg(unix)]
            let mut builder = builder;
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            match builder.create(&ruta) {
                Ok(()) => return Ok(Self(ruta)),
                Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(err) => return Err(persistencia(err)),
            }
        }
    }
}

impl Drop for DirectorioSonda {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn activar_wal(conn: &Connection) -> Result<(), AppError> {
    db::configurar_pragmas(conn).map_err(error_lectura)?;
    let modo: String = conn
        .query_row("PRAGMA journal_mode", [], |fila| fila.get(0))
        .map_err(error_lectura)?;
    if modo != "wal" {
        return Err(persistencia(format!(
            "No se pudo activar WAL en la base de datos: {modo}"
        )));
    }
    conn.query_row("SELECT count(*) FROM sqlite_master", [], |fila| {
        fila.get::<_, i64>(0)
    })
    .map_err(error_lectura)?;
    Ok(())
}

struct ConexionAbierta {
    conn: Connection,
    wal_pendiente: bool,
    checkpoint_protegido: bool,
    sonda: Option<DirectorioSonda>,
}

fn abrir_existente(ruta: &Path) -> Result<ConexionAbierta, AppError> {
    let mut archivo = fs::File::open(ruta).map_err(persistencia)?;
    if !archivo.metadata().map_err(persistencia)?.is_file() {
        return Err(persistencia(
            "La ruta de base de datos no es un archivo regular",
        ));
    }
    let mut cabecera = [0_u8; 100];
    let mut leidos = 0;
    while leidos < cabecera.len() {
        match archivo.read(&mut cabecera[leidos..]) {
            Ok(0) => break,
            Ok(n) => leidos += n,
            Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(err) => return Err(persistencia(err)),
        }
    }
    if leidos < cabecera.len() || &cabecera[..16] != b"SQLite format 3\0" {
        return Err(corrupta("Cabecera SQLite inválida o truncada"));
    }
    let tam_pagina = match u16::from_be_bytes([cabecera[16], cabecera[17]]) {
        1 => 65536,
        n => usize::from(n),
    };
    if !(512..=65536).contains(&tam_pagina) || !tam_pagina.is_power_of_two() {
        return Err(corrupta("Tamaño de página SQLite inválido"));
    }
    drop(archivo);
    let wal = auxiliar(ruta, "-wal");
    let shm = auxiliar(ruta, "-shm");
    let mut wal_existe = false;
    for (aux, es_wal) in [(&wal, true), (&shm, false)] {
        match fs::symlink_metadata(aux) {
            Ok(metadata) if metadata.file_type().is_file() => {
                if es_wal {
                    wal_existe = true;
                }
            }
            Ok(_) => {
                return Err(persistencia(format!(
                    "Auxiliar SQLite no es archivo regular: {}",
                    aux.display()
                )));
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(persistencia(err)),
        }
    }
    if wal_existe {
        validar_wal(&wal, tam_pagina)?;
    }
    drop(
        fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(ruta)
            .map_err(persistencia)?,
    );
    if wal_existe {
        drop(
            fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(&wal)
                .map_err(persistencia)?,
        );
    }
    let sonda = DirectorioSonda::crear()?;
    let copia = sonda.0.join("sgt.db");
    fs::copy(ruta, &copia).map_err(persistencia)?;
    if wal_existe {
        fs::copy(&wal, auxiliar(&copia, "-wal")).map_err(persistencia)?;
    }
    let mut conn = Connection::open_with_flags(&copia, rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE)
        .map_err(error_lectura)?;
    db::configurar_pragmas_lectura(&conn).map_err(error_lectura)?;
    db::verificar_integridad_fisica(&conn).map_err(|err| match err {
        db::ErrorIntegridad::Sqlite(err) => error_lectura(err),
        db::ErrorIntegridad::Corrupta(detalle) => corrupta(detalle),
    })?;
    let mut inutilizable = false;
    match leer_snapshot(&mut conn, &mut inutilizable) {
        Ok(_) => {}
        Err(err) if err.codigo == CodigoError::BaseDatosCorrupta && !inutilizable => {
            return Ok(ConexionAbierta {
                conn,
                wal_pendiente: false,
                checkpoint_protegido: false,
                sonda: Some(sonda),
            });
        }
        Err(err) => return Err(err),
    }
    drop(conn);
    drop(sonda);
    let es_wal = cabecera[18] == 2 || cabecera[19] == 2;
    let conn = Connection::open_with_flags(ruta, rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE)
        .map_err(error_lectura)?;
    if es_wal {
        conn.set_db_config(DbConfig::SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE, true)
            .map_err(error_lectura)?;
    }
    db::configurar_pragmas_lectura(&conn).map_err(error_lectura)?;
    if es_wal {
        conn.query_row("SELECT count(*) FROM sqlite_master", [], |fila| {
            fila.get::<_, i64>(0)
        })
        .map_err(error_lectura)?;
    }
    Ok(ConexionAbierta {
        conn,
        wal_pendiente: !es_wal,
        checkpoint_protegido: es_wal,
        sonda: None,
    })
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
    /// Se comprueba la integridad física antes de activar WAL.
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
        let abierta = if existe {
            abrir_existente(ruta)?
        } else {
            let temporal = preparar_temporal(ruta)?;
            match fs::hard_link(&temporal.0, ruta) {
                Ok(()) => match abrir_existente(ruta) {
                    Ok(mut abierta) => {
                        if let Err(err) = activar_wal(&abierta.conn) {
                            drop(abierta);
                            fs::remove_file(ruta).map_err(persistencia)?;
                            return Err(err);
                        }
                        abierta.wal_pendiente = false;
                        abierta.checkpoint_protegido = false;
                        abierta
                    }
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
        let ruta_sonda = abierta.sonda.as_ref().map(|_| ruta.to_path_buf());
        Ok(Self {
            conn: abierta.conn,
            inutilizable: false,
            wal_pendiente: abierta.wal_pendiente,
            checkpoint_protegido: abierta.checkpoint_protegido,
            sonda: abierta.sonda,
            ruta_sonda,
            #[cfg(feature = "test-fault-injection")]
            fallo_escritura: None,
        })
    }

    /// Programa un único fallo en el punto indicado de la próxima escritura.
    #[cfg(feature = "test-fault-injection")]
    pub fn inyectar_fallo_escritura(&mut self, punto: PuntoFalloEscritura) {
        self.fallo_escritura = Some(punto);
    }

    fn comprobar_conexion(&mut self) -> Result<(), AppError> {
        if self.inutilizable || !self.conn.is_autocommit() {
            self.inutilizable = true;
            return Err(persistencia("conexión con estado transaccional incierto"));
        }
        Ok(())
    }

    fn refrescar_sonda(&mut self) -> Result<(), AppError> {
        if let Some(ruta) = self.ruta_sonda.as_ref() {
            let abierta = abrir_existente(ruta)?;
            self.conn = abierta.conn;
            self.sonda = abierta.sonda;
            self.wal_pendiente = abierta.wal_pendiente;
            self.checkpoint_protegido = abierta.checkpoint_protegido;
            if self.sonda.is_none() {
                self.ruta_sonda = None;
            }
        }
        Ok(())
    }

    fn preparar_sesion(&mut self) -> Result<(), AppError> {
        if self.wal_pendiente {
            activar_wal(&self.conn)?;
            self.wal_pendiente = false;
        } else {
            self.conn
                .execute_batch("PRAGMA synchronous = NORMAL;")
                .map_err(error_lectura)?;
        }
        Ok(())
    }

    fn permitir_checkpoint(&mut self) -> Result<(), AppError> {
        if self.checkpoint_protegido {
            self.conn
                .set_db_config(DbConfig::SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE, false)
                .map_err(persistencia)?;
            self.checkpoint_protegido = false;
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
                    || mensaje.starts_with("no such column:")
                    || mensaje.starts_with("foreign key mismatch")
                    || mensaje.starts_with("malformed database schema")) =>
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

fn leer_snapshot(
    conn: &mut Connection,
    inutilizable: &mut bool,
) -> Result<SistemaTurnos, AppError> {
    let tx = conn.transaction().map_err(error_lectura)?;
    let resultado = (|| {
        let mut stmt = tx
            .prepare("SELECT version FROM schema_migrations ORDER BY version")
            .map_err(error_lectura)?;
        let versiones = stmt
            .query_map([], |fila| fila.get::<_, i64>(0))
            .map_err(error_lectura)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(error_lectura)?;
        if versiones != [1] {
            return Err(corrupta(format!(
                "Versiones de esquema incompatibles: {versiones:?}"
            )));
        }
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
                *inutilizable = true;
                Err(persistencia(format!("{err:?}; rollback: {rollback}")))
            }
        },
    };
    if !conn.is_autocommit() {
        *inutilizable = true;
        return Err(persistencia("Estado transaccional incierto tras cargar"));
    }
    resultado
}

impl Repositorio for RepositorioSqlite {
    fn cargar(&mut self) -> Result<SistemaTurnos, AppError> {
        self.comprobar_conexion()?;
        self.refrescar_sonda()?;
        let resultado = leer_snapshot(&mut self.conn, &mut self.inutilizable);
        let sistema = resultado?;
        self.preparar_sesion()?;
        self.permitir_checkpoint()?;
        Ok(sistema)
    }

    fn guardar(&mut self, cambios: &Cambios) -> Result<(), AppError> {
        self.comprobar_conexion()?;
        let ultima = secuencia_sql(cambios.ultima_secuencia)?;
        if self.sonda.is_some() {
            self.cargar()?;
        }
        self.preparar_sesion()?;
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
            #[cfg(feature = "test-fault-injection")]
            comprobar_fallo(
                &mut self.fallo_escritura,
                PuntoFalloEscritura::DespuesDeTurnos,
            )?;
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
            #[cfg(feature = "test-fault-injection")]
            comprobar_fallo(
                &mut self.fallo_escritura,
                PuntoFalloEscritura::DespuesDeVentanillas,
            )?;
            let count = tx
                .execute(
                    "UPDATE secuencias SET ultimo_valor=?1 WHERE clave='turno'",
                    [ultima],
                )
                .map_err(persistencia)?;
            if count != 1 {
                return Err(persistencia("No existe la secuencia de turnos"));
            }
            #[cfg(feature = "test-fault-injection")]
            comprobar_fallo(
                &mut self.fallo_escritura,
                PuntoFalloEscritura::AntesDeCommit,
            )?;
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
        resultado?;
        self.permitir_checkpoint()?;
        Ok(())
    }

    #[cfg(feature = "test-fault-injection")]
    fn inyectar_fallo_escritura(&mut self, punto: PuntoFalloEscritura) {
        self.fallo_escritura = Some(punto);
    }
}
