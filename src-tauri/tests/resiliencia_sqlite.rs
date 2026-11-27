//! Suite dedicada de resiliencia en disco para SQLite (`#13`: `CV-07`, `CV-08`, `CV-09`, `RD-03`).
//!
//! Verifica:
//! - `CV-07`: recuperación real desde `-wal` tras terminación abrupta de proceso
//!   con frames pendientes y cobertura de los cuatro estados de turno,
//!   ventanillas ocupadas/libres/inhabilitadas y secuencia.
//! - `CV-08`: reinicios limpios y abruptos combinados, preservando huecos donde
//!   `ultima_secuencia > MAX(secuencia)` sin reutilizar jamás IDs ni secuencias.
//! - `CV-09` (bajo `--features test-fault-injection`): rollback limpio en los
//!   tres puntos de [`PuntoFalloEscritura`], verificado mediante segunda conexión,
//!   reapertura completa y reintento exitoso en la misma instancia.
//! - `RD-03` físico: rechazo con `BaseDatosCorrupta` ante archivo vacío,
//!   truncado, basura binaria, corrupción B-tree en modos `DELETE` y `WAL`,
//!   corrupción en `-wal` (magia, checksum/payload, truncado), WAL reciclado
//!   tras `wal_checkpoint(RESTART)`, violación de llave foránea y `FK mismatch`,
//!   preservando bytes idénticos de DB/WAL/SHM y sin sidecars transitorios.
//! - `RD-03` lógico: rechazo con `BaseDatosCorrupta` ante `schema_migrations`
//!   vacía/desconocida/faltante, `secuencias` ausente, cero ventanillas activas,
//!   referencias inválidas a turnos `EN_ESPERA`/`ATENDIDO` y turno `EN_ATENCION`
//!   sin reflejo en `ventanillas`, garantizando ausencia de mutaciones y estado
//!   autocommit mediante reutilización de la instancia.

use std::collections::HashSet;
use std::ffi::{CStr, c_char, c_int, c_void};
use std::fs;
use std::mem::{align_of, size_of};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{TimeZone, Utc};
use gestor_de_turnos_lib::db;
use gestor_de_turnos_lib::models::{CodigoError, EstadoTurno, Turno, VentanillaId};
#[cfg(feature = "test-fault-injection")]
use gestor_de_turnos_lib::repository::PuntoFalloEscritura;
use gestor_de_turnos_lib::repository::{Cambios, Repositorio, RepositorioSqlite};
use rusqlite::{Connection, OpenFlags, ffi};

static CONTADOR_RUTAS: AtomicU64 = AtomicU64::new(0);

const ENV_CHILD_MODO: &str = "SGT_RESILIENCIA_CHILD_MODO";
const ENV_CHILD_DB: &str = "SGT_RESILIENCIA_CHILD_DB";

struct DirectorioPrueba {
    raiz: PathBuf,
    ruta_db: PathBuf,
}

impl DirectorioPrueba {
    fn nuevo(etiqueta: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("reloj posterior a UNIX_EPOCH")
            .as_nanos();
        let seq = CONTADOR_RUTAS.fetch_add(1, Ordering::Relaxed);
        let raiz = std::env::temp_dir().join(format!(
            "sgt_resiliencia_test_{etiqueta}_{}_{nanos}_{seq}",
            std::process::id()
        ));
        let ruta_db = raiz.join("sgt.db");
        Self { raiz, ruta_db }
    }

    fn ruta(&self) -> &Path {
        &self.ruta_db
    }

    fn ruta_wal(&self) -> PathBuf {
        PathBuf::from(format!("{}-wal", self.ruta_db.display()))
    }

    fn ruta_shm(&self) -> PathBuf {
        PathBuf::from(format!("{}-shm", self.ruta_db.display()))
    }

    fn copiar_tripleta_a(&self, destino: &Self) {
        fs::create_dir_all(&destino.raiz).expect("debe crear directorio destino de tripleta");
        fs::copy(self.ruta(), destino.ruta()).expect("debe copiar sgt.db");
        fs::copy(self.ruta_wal(), destino.ruta_wal()).expect("debe copiar sgt.db-wal");
        fs::copy(self.ruta_shm(), destino.ruta_shm()).expect("debe copiar sgt.db-shm");
    }
}

impl Drop for DirectorioPrueba {
    fn drop(&mut self) {
        for sufijo in ["", "-wal", "-shm", "-journal"] {
            let archivo = PathBuf::from(format!("{}{sufijo}", self.ruta_db.display()));
            let _ = fs::remove_file(archivo);
        }
        let _ = fs::remove_dir_all(&self.raiz);
    }
}

fn instante_fijo(segundo: u32) -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 11, 26, 15, 0, segundo)
        .single()
        .expect("fecha UTC válida")
}

fn abrir_conexion_auxiliar(ruta: &Path) -> Connection {
    db::abrir_conexion_configurada(ruta).expect("debe abrir conexión auxiliar configurada")
}

fn abrir_solo_lectura_sin_wal(ruta: &Path) -> Connection {
    Connection::open_with_flags(
        ruta,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .expect("debe abrir copia aislada en solo lectura")
}

fn leer_ultima_secuencia_sql(conn: &Connection) -> i64 {
    conn.query_row(
        "SELECT ultimo_valor FROM secuencias WHERE clave = 'turno'",
        [],
        |r| r.get(0),
    )
    .expect("debe consultar ultimo_valor")
}

fn leer_ventanillas_sql(conn: &Connection) -> Vec<(String, i64, Option<String>)> {
    let mut stmt = conn
        .prepare("SELECT id, habilitada, turno_actual_id FROM ventanillas ORDER BY id")
        .expect("debe preparar consulta de ventanillas");
    stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .expect("debe consultar ventanillas")
        .collect::<Result<Vec<_>, _>>()
        .expect("debe recolectar ventanillas")
}

fn leer_turnos_sql(conn: &Connection) -> Vec<(String, i64, String, String, Option<String>)> {
    let mut stmt = conn
        .prepare(
            "SELECT id, secuencia, fecha_hora_registro, estado, ventanilla_asignada
             FROM turnos ORDER BY secuencia",
        )
        .expect("debe preparar consulta de turnos");
    stmt.query_map([], |r| {
        Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
    })
    .expect("debe consultar turnos")
    .collect::<Result<Vec<_>, _>>()
    .expect("debe recolectar turnos")
}

#[derive(Debug, PartialEq, Eq)]
struct SnapshotSql {
    tablas: Vec<String>,
    migraciones: Option<Vec<(i64, String)>>,
    secuencias: Option<Vec<(String, i64)>>,
    ventanillas: Vec<(String, i64, Option<String>)>,
    turnos: Vec<(String, i64, String, String, Option<String>)>,
}

fn capturar_snapshot_sql(conn: &Connection) -> SnapshotSql {
    let mut stmt_tablas = conn
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
        .expect("debe consultar sqlite_master");
    let tablas = stmt_tablas
        .query_map([], |r| r.get::<_, String>(0))
        .expect("debe mapear tablas")
        .collect::<Result<Vec<_>, _>>()
        .expect("debe recolectar tablas");

    let migraciones = if tablas.iter().any(|t| t == "schema_migrations") {
        let mut stmt = conn
            .prepare("SELECT version, nombre FROM schema_migrations ORDER BY version")
            .expect("debe consultar schema_migrations");
        let filas = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .expect("debe mapear schema_migrations")
            .collect::<Result<Vec<_>, _>>()
            .expect("debe recolectar schema_migrations");
        Some(filas)
    } else {
        None
    };

    let secuencias = if tablas.iter().any(|t| t == "secuencias") {
        let mut stmt = conn
            .prepare("SELECT clave, ultimo_valor FROM secuencias ORDER BY clave")
            .expect("debe consultar secuencias");
        let filas = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .expect("debe mapear secuencias")
            .collect::<Result<Vec<_>, _>>()
            .expect("debe recolectar secuencias");
        Some(filas)
    } else {
        None
    };

    SnapshotSql {
        tablas,
        migraciones,
        secuencias,
        ventanillas: leer_ventanillas_sql(conn),
        turnos: leer_turnos_sql(conn),
    }
}

fn ejecutar_subproceso_cierre_abrupto(modo: &str, ruta_db: &Path) {
    let exe = std::env::current_exe().expect("debe localizar ejecutable de pruebas");
    let output = Command::new(exe)
        .args([
            "--exact",
            "aux_subproceso_cierre_abrupto_wal",
            "--nocapture",
        ])
        .env(ENV_CHILD_MODO, modo)
        .env(ENV_CHILD_DB, ruta_db)
        .output()
        .expect("debe lanzar subproceso de cierre abrupto");
    assert!(
        output.status.success(),
        "subproceso '{modo}' falló ({}): stdout={} stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn aux_subproceso_cierre_abrupto_wal() {
    let Ok(modo) = std::env::var(ENV_CHILD_MODO) else {
        return;
    };
    let ruta_db = PathBuf::from(
        std::env::var_os(ENV_CHILD_DB).expect("debe especificarse ruta DB para subproceso"),
    );

    match modo.as_str() {
        "cv07" | "cv08_abrupto_con_hueco" | "btree_corrupto_wal_preparar" | "solo_t0001_wal" => {
            let mut repo =
                RepositorioSqlite::abrir(&ruta_db).expect("subproceso debe abrir repositorio");
            let mut sistema = repo.cargar().expect("subproceso debe cargar estado");

            match modo.as_str() {
                "cv07" => {
                    let t1 = sistema
                        .registrar_turno_en(instante_fijo(1))
                        .expect("T-0001");
                    let t2 = sistema
                        .registrar_turno_en(instante_fijo(2))
                        .expect("T-0002");
                    let t3 = sistema
                        .registrar_turno_en(instante_fijo(3))
                        .expect("T-0003");
                    let t4 = sistema
                        .registrar_turno_en(instante_fijo(4))
                        .expect("T-0004");
                    repo.guardar(&Cambios {
                        turnos: vec![t1, t2, t3, t4],
                        ventanillas: vec![],
                        ultima_secuencia: sistema.ultima_secuencia(),
                    })
                    .expect("debe confirmar turnos iniciales");

                    let t1_atencion = sistema
                        .asignar_siguiente(VentanillaId::V01)
                        .expect("asignar T-0001 a V01");
                    repo.guardar(&Cambios {
                        turnos: vec![t1_atencion],
                        ventanillas: vec![sistema.ventanilla(VentanillaId::V01).clone()],
                        ultima_secuencia: sistema.ultima_secuencia(),
                    })
                    .expect("debe confirmar T-0001 EN_ATENCION");

                    let t1_atendido = sistema
                        .finalizar_atencion(VentanillaId::V01)
                        .expect("finalizar T-0001");
                    let t2_cancelado = sistema.cancelar_turno("T-0002").expect("cancelar T-0002");
                    let t3_atencion = sistema
                        .asignar_siguiente(VentanillaId::V02)
                        .expect("asignar T-0003 a V02");
                    let v03_off = sistema
                        .cambiar_estado_ventanilla(VentanillaId::V03, false)
                        .expect("inhabilitar V03");
                    repo.guardar(&Cambios {
                        turnos: vec![t1_atendido, t2_cancelado, t3_atencion],
                        ventanillas: vec![
                            sistema.ventanilla(VentanillaId::V01).clone(),
                            sistema.ventanilla(VentanillaId::V02).clone(),
                            v03_off,
                        ],
                        ultima_secuencia: sistema.ultima_secuencia(),
                    })
                    .expect("debe confirmar todos los estados y ventanilla deshabilitada en WAL");
                }
                "cv08_abrupto_con_hueco" => {
                    assert_eq!(sistema.ultima_secuencia(), 10);
                    let t11 = sistema
                        .registrar_turno_en(instante_fijo(20))
                        .expect("T-0011");
                    assert_eq!(t11.id, "T-0011");
                    let t12 = sistema
                        .registrar_turno_en(instante_fijo(21))
                        .expect("T-0012");
                    assert_eq!(t12.id, "T-0012");
                    repo.guardar(&Cambios {
                        turnos: vec![t11, t12],
                        ventanillas: vec![],
                        ultima_secuencia: sistema.ultima_secuencia(),
                    })
                    .expect("debe guardar T-0011 y T-0012");

                    let t3_cancelado = sistema.cancelar_turno("T-0003").expect("cancelar T-0003");
                    let t4_asignado = sistema
                        .asignar_siguiente(VentanillaId::V02)
                        .expect("asignar T-0004");
                    repo.guardar(&Cambios {
                        turnos: vec![t4_asignado],
                        ventanillas: vec![sistema.ventanilla(VentanillaId::V02).clone()],
                        ultima_secuencia: sistema.ultima_secuencia(),
                    })
                    .expect("debe guardar T-0004 en atención");

                    let t4_atendido = sistema
                        .finalizar_atencion(VentanillaId::V02)
                        .expect("finalizar T-0004");
                    repo.guardar(&Cambios {
                        turnos: vec![t3_cancelado, t4_atendido],
                        ventanillas: vec![sistema.ventanilla(VentanillaId::V02).clone()],
                        ultima_secuencia: 25,
                    })
                    .expect("debe confirmar hueco ultima_secuencia=25 en WAL");
                }
                "btree_corrupto_wal_preparar" => {
                    let t6 = sistema
                        .registrar_turno_en(instante_fijo(6))
                        .expect("T-0006");
                    repo.guardar(&Cambios {
                        turnos: vec![t6],
                        ventanillas: vec![],
                        ultima_secuencia: sistema.ultima_secuencia(),
                    })
                    .expect("debe guardar T-0006 en WAL");
                }
                "solo_t0001_wal" => {
                    let t1 = sistema
                        .registrar_turno_en(instante_fijo(1))
                        .expect("T-0001");
                    repo.guardar(&Cambios {
                        turnos: vec![t1],
                        ventanillas: vec![],
                        ultima_secuencia: sistema.ultima_secuencia(),
                    })
                    .expect("debe confirmar T-0001 solo en WAL");
                }
                _ => unreachable!(),
            }

            let ruta_wal = PathBuf::from(format!("{}-wal", ruta_db.display()));
            let tam_wal = fs::metadata(&ruta_wal)
                .expect("debe existir -wal con frames pendientes")
                .len();
            assert!(
                tam_wal > 32,
                "el archivo -wal debe contener frames confirmados pendientes (len={tam_wal})"
            );

            // Simula terminación abrupta sin ejecutar sqlite3_close ni checkpoint de cierre.
            std::mem::forget(repo);
            std::process::exit(0);
        }
        "fk_invalida_wal" => {
            {
                let mut repo =
                    RepositorioSqlite::abrir(&ruta_db).expect("debe crear BD limpia previa");
                let _ = repo.cargar().expect("debe cargar inicial");
            }
            let conn = abrir_conexion_auxiliar(&ruta_db);
            conn.execute_batch(
                "PRAGMA foreign_keys = OFF;
                 BEGIN IMMEDIATE;
                 UPDATE secuencias SET ultimo_valor = 1 WHERE clave = 'turno';
                 INSERT INTO turnos (id, secuencia, fecha_hora_registro, estado, ventanilla_asignada)
                 VALUES ('T-0001', 1, '2026-11-26T15:00:01.000Z', 'ATENDIDO', 'V99');
                 COMMIT;",
            )
            .expect("debe confirmar FK inválida en WAL pendiente");

            let ruta_wal = PathBuf::from(format!("{}-wal", ruta_db.display()));
            let ruta_shm = PathBuf::from(format!("{}-shm", ruta_db.display()));
            assert!(
                fs::metadata(&ruta_wal).expect("debe existir -wal").len() > 32,
                "el -wal debe tener frames confirmados"
            );
            assert!(ruta_shm.exists(), "debe existir -shm");

            std::mem::forget(conn);
            std::process::exit(0);
        }
        "migracion_2_wal" => {
            {
                let mut repo =
                    RepositorioSqlite::abrir(&ruta_db).expect("debe crear BD limpia previa");
                let _ = repo.cargar().expect("debe cargar inicial");
            }
            let conn = abrir_conexion_auxiliar(&ruta_db);
            conn.execute_batch(
                "BEGIN IMMEDIATE;
                 UPDATE schema_migrations SET version = 2, nombre = '002_ajena' WHERE version = 1;
                 UPDATE secuencias SET ultimo_valor = 1 WHERE clave = 'turno';
                 INSERT INTO turnos (id, secuencia, fecha_hora_registro, estado, ventanilla_asignada)
                 VALUES ('T-0001', 1, '2026-11-26T15:00:01.000Z', 'EN_ESPERA', NULL);
                 COMMIT;",
            )
            .expect("debe confirmar migración [2] en WAL pendiente");

            let ruta_wal = PathBuf::from(format!("{}-wal", ruta_db.display()));
            assert!(
                fs::metadata(&ruta_wal).expect("debe existir -wal").len() > 32,
                "el -wal debe tener frames confirmados"
            );

            std::mem::forget(conn);
            std::process::exit(0);
        }
        "wal_reciclado_restart" => {
            {
                let mut repo =
                    RepositorioSqlite::abrir(&ruta_db).expect("debe crear BD limpia previa");
                let _ = repo.cargar().expect("debe cargar inicial");
            }
            let conn = abrir_conexion_auxiliar(&ruta_db);
            // Varias transacciones individuales para garantizar un WAL con muchos frames.
            for seq in 1..=15_i64 {
                let id = format!("T-{seq:04}");
                let fecha = format!("2026-11-26T15:00:{seq:02}.000Z");
                conn.execute_batch("BEGIN IMMEDIATE;")
                    .expect("debe iniciar transacción");
                conn.execute(
                    "UPDATE secuencias SET ultimo_valor = ?1 WHERE clave = 'turno'",
                    [seq],
                )
                .expect("debe actualizar secuencia");
                conn.execute(
                    "INSERT INTO turnos (id, secuencia, fecha_hora_registro, estado, ventanilla_asignada)
                     VALUES (?1, ?2, ?3, 'EN_ESPERA', NULL)",
                    rusqlite::params![id, seq, fecha],
                )
                .expect("debe insertar turno");
                conn.execute_batch("COMMIT;").expect("debe confirmar turno");
            }

            let ruta_wal = PathBuf::from(format!("{}-wal", ruta_db.display()));
            let wal_previo = fs::read(&ruta_wal).expect("debe leer -wal previo al checkpoint");
            assert!(
                wal_previo.len() > 32 + (24 + 4096) * 5,
                "el -wal inicial debe contener múltiples frames (len={})",
                wal_previo.len()
            );

            let (busy, log, ckpt): (i64, i64, i64) = conn
                .query_row("PRAGMA wal_checkpoint(RESTART);", [], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?))
                })
                .expect("debe ejecutar wal_checkpoint(RESTART)");
            assert_eq!(busy, 0);
            assert!(log > 5);
            assert_eq!(ckpt, log);

            // Commit menor: solo 1 transacción, sobreescribe los primeros frames con un salt nuevo
            // y deja al final del -wal los frames residuales del salt anterior.
            conn.execute_batch(
                "BEGIN IMMEDIATE;
                 UPDATE secuencias SET ultimo_valor = 16 WHERE clave = 'turno';
                 INSERT INTO turnos (id, secuencia, fecha_hora_registro, estado, ventanilla_asignada)
                 VALUES ('T-0016', 16, '2026-11-26T15:00:16.000Z', 'EN_ESPERA', NULL);
                 COMMIT;",
            )
            .expect("debe confirmar commit menor tras RESTART");

            let wal_reciclado = fs::read(&ruta_wal).expect("debe leer -wal reciclado");
            assert!(
                wal_reciclado.len() >= wal_previo.len(),
                "wal_checkpoint(RESTART) no debe truncar el archivo -wal"
            );
            let tam_pagina = usize::from(u16::from_be_bytes([
                fs::read(&ruta_db).expect("db")[16],
                fs::read(&ruta_db).expect("db")[17],
            ]));
            let frame_len = 24 + tam_pagina;
            let num_frames = (wal_reciclado.len() - 32) / frame_len;
            assert!(num_frames >= 3);
            let checkpoint_seq =
                u32::from_be_bytes(wal_reciclado[12..16].try_into().expect("checkpoint"));
            assert!(
                checkpoint_seq > 0,
                "el WAL reciclado debe tener checkpoint_seq > 0"
            );
            let salt_actual = &wal_reciclado[16..24];
            let salt_primer_frame = &wal_reciclado[32 + 8..32 + 16];
            let inicio_ultimo = 32 + (num_frames - 1) * frame_len;
            let salt_ultimo_frame = &wal_reciclado[inicio_ultimo + 8..inicio_ultimo + 16];
            assert_eq!(
                salt_primer_frame, salt_actual,
                "el primer frame debe pertenecer al salt vigente"
            );
            assert_ne!(
                salt_ultimo_frame, salt_actual,
                "la cola del WAL reciclado debe conservar frames del salt anterior"
            );

            std::mem::forget(conn);
            std::process::exit(0);
        }
        "vfs_wal_cabecera_32_sin_frames" => {
            let armado = Arc::new(AtomicBool::new(false));
            let corte_aplicado = Arc::new(AtomicBool::new(false));
            let umbral_offset = Arc::new(AtomicI64::new(32));
            let (_guardia, _registro) = GuardiaObservadorVfs::instalar_con_corte_wal(
                ruta_db
                    .parent()
                    .expect("ruta_db debe tener directorio padre"),
                ConfigCorteWal {
                    armado: Arc::clone(&armado),
                    corte_aplicado: Arc::clone(&corte_aplicado),
                    umbral_offset: Arc::clone(&umbral_offset),
                    bytes_corte: 0,
                },
            );
            let mut repo =
                RepositorioSqlite::abrir(&ruta_db).expect("debe abrir BD limpia inicial");
            let mut sistema = repo.cargar().expect("debe cargar estado inicial");

            armado.store(true, Ordering::SeqCst);
            let t1 = sistema
                .registrar_turno_en(instante_fijo(1))
                .expect("T-0001");
            let err = repo
                .guardar(&Cambios {
                    turnos: vec![t1],
                    ventanillas: vec![],
                    ultima_secuencia: sistema.ultima_secuencia(),
                })
                .expect_err("el corte VFS tras la cabecera de 32 bytes debe abortar guardar()");
            assert_eq!(err.codigo, CodigoError::ErrorPersistencia);
            assert!(
                corte_aplicado.load(Ordering::SeqCst),
                "el VFS debió interceptar la escritura del primer frame"
            );

            let ruta_wal = PathBuf::from(format!("{}-wal", ruta_db.display()));
            let tam_wal = fs::metadata(&ruta_wal)
                .expect("debe existir -wal con cabecera de 32 bytes")
                .len();
            assert_eq!(
                tam_wal, 32,
                "el -wal debe quedar exactamente en 32 bytes de cabecera válida sin frames"
            );

            std::mem::forget(repo);
            std::process::exit(0);
        }
        "vfs_wal_corte_parcial_12_tras_t0001" => {
            let armado = Arc::new(AtomicBool::new(false));
            let corte_aplicado = Arc::new(AtomicBool::new(false));
            let umbral_offset = Arc::new(AtomicI64::new(0));
            let (_guardia, _registro) = GuardiaObservadorVfs::instalar_con_corte_wal(
                ruta_db
                    .parent()
                    .expect("ruta_db debe tener directorio padre"),
                ConfigCorteWal {
                    armado: Arc::clone(&armado),
                    corte_aplicado: Arc::clone(&corte_aplicado),
                    umbral_offset: Arc::clone(&umbral_offset),
                    bytes_corte: 12,
                },
            );
            let mut repo =
                RepositorioSqlite::abrir(&ruta_db).expect("debe abrir BD limpia inicial");
            let mut sistema = repo.cargar().expect("debe cargar estado inicial");

            let t1 = sistema
                .registrar_turno_en(instante_fijo(1))
                .expect("T-0001");
            repo.guardar(&Cambios {
                turnos: vec![t1],
                ventanillas: vec![],
                ultima_secuencia: sistema.ultima_secuencia(),
            })
            .expect("debe confirmar T-0001 en -wal antes del corte");

            let ruta_wal = PathBuf::from(format!("{}-wal", ruta_db.display()));
            let tam_confirmado = fs::metadata(&ruta_wal)
                .expect("debe existir -wal con T-0001 confirmada")
                .len();
            assert!(
                tam_confirmado > 32,
                "T-0001 debe haber dejado frames confirmados en -wal (len={tam_confirmado})"
            );

            umbral_offset.store(
                i64::try_from(tam_confirmado).expect("tamaño WAL cabe en i64"),
                Ordering::SeqCst,
            );
            armado.store(true, Ordering::SeqCst);

            let t2 = sistema
                .registrar_turno_en(instante_fijo(2))
                .expect("T-0002");
            let err = repo
                .guardar(&Cambios {
                    turnos: vec![t2],
                    ventanillas: vec![],
                    ultima_secuencia: sistema.ultima_secuencia(),
                })
                .expect_err("el corte parcial de 12 bytes vía VFS debe abortar guardar(T-0002)");
            assert_eq!(err.codigo, CodigoError::ErrorPersistencia);
            assert!(
                corte_aplicado.load(Ordering::SeqCst),
                "el VFS debió aplicar el corte parcial de 12 bytes"
            );

            let tam_tras_corte = fs::metadata(&ruta_wal)
                .expect("debe existir -wal tras corte parcial")
                .len();
            assert_eq!(
                tam_tras_corte,
                tam_confirmado + 12,
                "el -wal debe terminar con un corte parcial exacto de 12 bytes tras T-0001"
            );

            std::mem::forget(repo);
            std::process::exit(0);
        }
        "regresion_i_spill_y_t0002" | "regresion_i_spill_sin_t0002" => {
            let mut repo =
                RepositorioSqlite::abrir(&ruta_db).expect("debe abrir BD limpia inicial");
            let mut sistema = repo.cargar().expect("debe cargar estado inicial");

            let t1 = sistema
                .registrar_turno_en(instante_fijo(1))
                .expect("T-0001");
            repo.guardar(&Cambios {
                turnos: vec![t1],
                ventanillas: vec![],
                ultima_secuencia: sistema.ultima_secuencia(),
            })
            .expect("debe confirmar T-0001");

            let ruta_wal = PathBuf::from(format!("{}-wal", ruta_db.display()));
            let tam_wal_t1 = fs::metadata(&ruta_wal)
                .expect("debe existir -wal tras T-0001")
                .len();
            assert!(tam_wal_t1 > 32);

            let total_validos: u64 = 50_000;
            let mut lote = Vec::with_capacity(usize::try_from(total_validos).expect("usize") + 1);
            for seq in 2..=(total_validos + 1) {
                lote.push(Turno {
                    id: format!("T-{seq:04}"),
                    secuencia: seq,
                    fecha_hora_registro: "2026-11-26T15:00:01.000Z".to_string(),
                    estado: EstadoTurno::EnEspera,
                    ventanilla_asignada: None,
                });
            }
            lote.push(Turno {
                id: String::new(),
                secuencia: total_validos + 2,
                fecha_hora_registro: "2026-11-26T15:00:01.000Z".to_string(),
                estado: EstadoTurno::EnEspera,
                ventanilla_asignada: None,
            });

            let err = repo
                .guardar(&Cambios {
                    turnos: lote,
                    ventanillas: vec![],
                    ultima_secuencia: total_validos + 2,
                })
                .expect_err(
                    "el id vacío al final del lote de ~50000 turnos debe fallar y revertir",
                );
            assert_eq!(err.codigo, CodigoError::ErrorPersistencia);

            let tam_wal_spill = fs::metadata(&ruta_wal)
                .expect("debe existir -wal tras rollback con spill")
                .len();
            assert!(
                tam_wal_spill > tam_wal_t1 + 50 * (24 + 4096),
                "el lote de 50000 turnos debe provocar spill real en -wal (t1={tam_wal_t1}, spill={tam_wal_spill})"
            );

            let mut sistema_tras_rollback = repo
                .cargar()
                .expect("cargar() tras rollback con spill debe restaurar estado consistente");
            assert_eq!(sistema_tras_rollback.ultima_secuencia(), 1);
            assert_eq!(sistema_tras_rollback.consultar_historial().len(), 1);

            if modo == "regresion_i_spill_y_t0002" {
                let t2 = sistema_tras_rollback
                    .registrar_turno_en(instante_fijo(2))
                    .expect("T-0002");
                assert_eq!(t2.id, "T-0002");
                repo.guardar(&Cambios {
                    turnos: vec![t2],
                    ventanillas: vec![],
                    ultima_secuencia: sistema_tras_rollback.ultima_secuencia(),
                })
                .expect("debe confirmar T-0002 en la misma instancia tras el rollback");

                let sistema_post_t2 = repo
                    .cargar()
                    .expect("cargar() tras guardar T-0002 debe confirmar secuencia 2");
                assert_eq!(sistema_post_t2.ultima_secuencia(), 2);
                assert_eq!(sistema_post_t2.consultar_historial().len(), 2);
            }

            std::mem::forget(repo);
            std::process::exit(0);
        }
        otro => panic!("modo desconocido de subproceso: {otro}"),
    }
}

#[test]
fn cv07_recuperacion_wal_real_tras_cierre_abrupto_con_frames_pendientes() {
    let dir = DirectorioPrueba::nuevo("cv07_wal_abrupto");
    ejecutar_subproceso_cierre_abrupto("cv07", dir.ruta());

    // Evidencia en disco antes de reabrir: `-wal` contiene frames confirmados no integrados.
    let tam_wal = fs::metadata(dir.ruta_wal())
        .expect("sgt.db-wal debe permanecer en disco tras cierre abrupto")
        .len();
    assert!(
        tam_wal > 32,
        "sgt.db-wal debe contener cabecera + frames pendientes (len={tam_wal})"
    );

    // Copiar únicamente sgt.db (sin -wal/-shm) para confirmar que los cambios no
    // habían sido integrados aún al archivo principal antes de reabrir.
    let copia_aislada = dir.raiz.join("solo_db_sin_wal.db");
    fs::copy(dir.ruta(), &copia_aislada).expect("debe copiar archivo principal aislado");
    {
        let conn_aislada = abrir_solo_lectura_sin_wal(&copia_aislada);
        let turnos_en_db: i64 = conn_aislada
            .query_row("SELECT COUNT(*) FROM turnos", [], |r| r.get(0))
            .expect("debe consultar turnos en copia aislada");
        let seq_en_db: i64 = leer_ultima_secuencia_sql(&conn_aislada);
        assert_eq!(
            turnos_en_db, 0,
            "los turnos confirmados deben residir aún solo en -wal antes de recuperar"
        );
        assert_eq!(
            seq_en_db, 0,
            "la secuencia confirmada debe residir aún solo en -wal antes de recuperar"
        );
    }
    let _ = fs::remove_file(&copia_aislada);

    // Al reabrir con RepositorioSqlite, SQLite reproduce los frames del WAL y recupera todo.
    {
        let mut repo = RepositorioSqlite::abrir(dir.ruta())
            .expect("RepositorioSqlite::abrir debe recuperar WAL tras cierre abrupto");
        let sistema = repo
            .cargar()
            .expect("debe cargar el 100% del estado recuperado del WAL");

        assert_eq!(sistema.ultima_secuencia(), 4);

        let v01 = sistema.ventanilla(VentanillaId::V01);
        assert!(v01.habilitada);
        assert_eq!(v01.turno_actual_id, None);

        let v02 = sistema.ventanilla(VentanillaId::V02);
        assert!(v02.habilitada);
        assert_eq!(v02.turno_actual_id.as_deref(), Some("T-0003"));

        let v03 = sistema.ventanilla(VentanillaId::V03);
        assert!(!v03.habilitada);
        assert_eq!(v03.turno_actual_id, None);

        let t1 = sistema.buscar_turno("T-0001").expect("T-0001");
        assert_eq!(t1.estado, EstadoTurno::Atendido);
        assert_eq!(t1.ventanilla_asignada, Some(VentanillaId::V01));

        let t2 = sistema.buscar_turno("T-0002").expect("T-0002");
        assert_eq!(t2.estado, EstadoTurno::Cancelado);
        assert_eq!(t2.ventanilla_asignada, None);

        let t3 = sistema.buscar_turno("T-0003").expect("T-0003");
        assert_eq!(t3.estado, EstadoTurno::EnAtencion);
        assert_eq!(t3.ventanilla_asignada, Some(VentanillaId::V02));

        let t4 = sistema.buscar_turno("T-0004").expect("T-0004");
        assert_eq!(t4.estado, EstadoTurno::EnEspera);
        assert_eq!(t4.ventanilla_asignada, None);
    }

    // Tras un cierre limpio posterior, el estado recuperado sigue intacto en disco.
    let mut repo_reabierto =
        RepositorioSqlite::abrir(dir.ruta()).expect("debe reabrir tras cierre limpio");
    let sistema_reabierto = repo_reabierto
        .cargar()
        .expect("debe conservar todos los turnos, ventanillas y secuencia");
    assert_eq!(sistema_reabierto.ultima_secuencia(), 4);
    assert_eq!(sistema_reabierto.consultar_historial().len(), 4);
    assert!(!sistema_reabierto.ventanilla(VentanillaId::V03).habilitada);
}

#[test]
fn cv08_reinicios_limpios_y_abruptos_con_huecos_no_reutilizan_secuencias() {
    let dir = DirectorioPrueba::nuevo("cv08_huecos_reinicios");

    // Ciclo 1 (cierre limpio): registra T-0001..T-0003, finaliza T-0001, cancela T-0002.
    {
        let mut repo = RepositorioSqlite::abrir(dir.ruta()).expect("debe abrir BD inicial");
        let mut sistema = repo.cargar().expect("debe cargar inicial");
        let t1 = sistema
            .registrar_turno_en(instante_fijo(1))
            .expect("T-0001");
        let t2 = sistema
            .registrar_turno_en(instante_fijo(2))
            .expect("T-0002");
        let t3 = sistema
            .registrar_turno_en(instante_fijo(3))
            .expect("T-0003");
        repo.guardar(&Cambios {
            turnos: vec![t1, t2, t3],
            ventanillas: vec![],
            ultima_secuencia: 3,
        })
        .expect("debe guardar T-0001..T-0003");

        let t1_asignado = sistema
            .asignar_siguiente(VentanillaId::V01)
            .expect("asignar T-0001");
        repo.guardar(&Cambios {
            turnos: vec![t1_asignado],
            ventanillas: vec![sistema.ventanilla(VentanillaId::V01).clone()],
            ultima_secuencia: sistema.ultima_secuencia(),
        })
        .expect("debe guardar T-0001 en atención");

        let t1_atendido = sistema
            .finalizar_atencion(VentanillaId::V01)
            .expect("finalizar T-0001");
        let t2_cancelado = sistema.cancelar_turno("T-0002").expect("cancelar T-0002");
        repo.guardar(&Cambios {
            turnos: vec![t1_atendido, t2_cancelado],
            ventanillas: vec![sistema.ventanilla(VentanillaId::V01).clone()],
            ultima_secuencia: sistema.ultima_secuencia(),
        })
        .expect("debe confirmar estados terminales");
    }

    // Ciclo 2 (cierre limpio con hueco): genera T-0004 y persiste ultima_secuencia=10 > MAX(4).
    {
        let mut repo = RepositorioSqlite::abrir(dir.ruta()).expect("debe reabrir ciclo 2");
        let mut sistema = repo.cargar().expect("debe cargar ciclo 2");
        assert_eq!(sistema.ultima_secuencia(), 3);

        let t4 = sistema
            .registrar_turno_en(instante_fijo(10))
            .expect("T-0004");
        assert_eq!(t4.id, "T-0004");
        assert_eq!(t4.secuencia, 4);
        repo.guardar(&Cambios {
            turnos: vec![t4],
            ventanillas: vec![],
            ultima_secuencia: 10,
        })
        .expect("debe guardar hueco ultima_secuencia=10");
    }

    // Ciclo 3 (cierre abrupto vía subproceso con otro hueco ultima_secuencia=25 en WAL).
    ejecutar_subproceso_cierre_abrupto("cv08_abrupto_con_hueco", dir.ruta());
    assert!(
        fs::metadata(dir.ruta_wal())
            .expect("debe existir -wal tras cierre abrupto")
            .len()
            > 32
    );

    // Ciclo 4 (recuperación tras cierre abrupto + operación sin registro que no altera secuencia).
    {
        let mut repo = RepositorioSqlite::abrir(dir.ruta()).expect("debe recuperar tras abrupto");
        let mut sistema = repo.cargar().expect("debe cargar estado con hueco 25");
        assert_eq!(sistema.ultima_secuencia(), 25);

        let v03_off = sistema
            .cambiar_estado_ventanilla(VentanillaId::V03, false)
            .expect("inhabilitar V03");
        repo.guardar(&Cambios {
            turnos: vec![],
            ventanillas: vec![v03_off],
            ultima_secuencia: sistema.ultima_secuencia(),
        })
        .expect("debe preservar ultima_secuencia=25 sin incrementar");
    }

    // Ciclo 5 (nuevo reinicio limpio): siguiente turno debe ser T-0026, sin colisionar ni reutilizar.
    {
        let mut repo = RepositorioSqlite::abrir(dir.ruta()).expect("debe reabrir ciclo 5");
        let mut sistema = repo.cargar().expect("debe cargar ciclo 5");
        assert_eq!(sistema.ultima_secuencia(), 25);

        let t26 = sistema
            .registrar_turno_en(instante_fijo(30))
            .expect("debe emitir T-0026");
        assert_eq!(t26.id, "T-0026");
        assert_eq!(t26.secuencia, 26);

        repo.guardar(&Cambios {
            turnos: vec![t26],
            ventanillas: vec![],
            ultima_secuencia: sistema.ultima_secuencia(),
        })
        .expect("debe guardar T-0026");
    }

    let mut repo_final = RepositorioSqlite::abrir(dir.ruta()).expect("debe reabrir final");
    let sistema_final = repo_final.cargar().expect("debe cargar final");
    assert_eq!(sistema_final.ultima_secuencia(), 26);

    let historial = sistema_final.consultar_historial();
    let ids: Vec<&str> = historial.iter().map(|t| t.id.as_str()).collect();
    let secuencias: Vec<u64> = historial.iter().map(|t| t.secuencia).collect();
    assert_eq!(
        ids,
        vec![
            "T-0001", "T-0002", "T-0003", "T-0004", "T-0011", "T-0012", "T-0026"
        ]
    );
    assert_eq!(secuencias, vec![1, 2, 3, 4, 11, 12, 26]);
    assert_eq!(ids.iter().collect::<HashSet<_>>().len(), ids.len());
    assert_eq!(
        secuencias.iter().collect::<HashSet<_>>().len(),
        secuencias.len()
    );
}

#[cfg(feature = "test-fault-injection")]
fn verificar_cv09_punto_fallo(etiqueta: &str, punto: PuntoFalloEscritura) {
    let preparar_lote = |repo: &mut RepositorioSqlite| {
        let mut sistema = repo.cargar().expect("debe cargar estado inicial");
        let t1 = sistema
            .registrar_turno_en(instante_fijo(1))
            .expect("T-0001");
        repo.guardar(&Cambios {
            turnos: vec![t1],
            ventanillas: vec![],
            ultima_secuencia: 1,
        })
        .expect("debe guardar T-0001");

        let mut borrador = sistema.clone();
        let t2 = borrador
            .registrar_turno_en(instante_fijo(2))
            .expect("T-0002 en borrador");
        let t1_asignado = borrador
            .asignar_siguiente(VentanillaId::V01)
            .expect("asignar T-0001 en borrador");
        let cambios = Cambios {
            turnos: vec![t1_asignado, t2],
            ventanillas: vec![borrador.ventanilla(VentanillaId::V01).clone()],
            ultima_secuencia: borrador.ultima_secuencia(),
        };
        (sistema, borrador, cambios)
    };

    // Fase A: fallo inyectado + verificación en segunda conexión + reapertura completa sin restos.
    {
        let dir = DirectorioPrueba::nuevo(&format!("cv09_{etiqueta}_reapertura"));
        let mut repo = RepositorioSqlite::abrir(dir.ruta()).expect("debe abrir BD");
        let (sistema_previo, _borrador, cambios) = preparar_lote(&mut repo);

        repo.inyectar_fallo_escritura(punto);
        let err = repo
            .guardar(&cambios)
            .expect_err("el fallo inyectado debe abortar guardar()");
        assert_eq!(err.codigo, CodigoError::ErrorPersistencia);

        {
            let conn_aux = abrir_conexion_auxiliar(dir.ruta());
            assert_eq!(leer_ultima_secuencia_sql(&conn_aux), 1);
            assert_eq!(
                leer_ventanillas_sql(&conn_aux),
                vec![
                    ("V01".to_string(), 1, None),
                    ("V02".to_string(), 1, None),
                    ("V03".to_string(), 1, None),
                ]
            );
            assert_eq!(
                leer_turnos_sql(&conn_aux),
                vec![(
                    "T-0001".to_string(),
                    1,
                    "2026-11-26T15:00:01.000Z".to_string(),
                    "EN_ESPERA".to_string(),
                    None,
                )]
            );
        }

        drop(repo);

        let mut reabierto =
            RepositorioSqlite::abrir(dir.ruta()).expect("debe reabrir tras rollback");
        let restaurado = reabierto
            .cargar()
            .expect("debe cargar estado previo intacto tras reapertura");
        assert_eq!(restaurado.ultima_secuencia(), 1);
        assert_eq!(restaurado.obtener_estado(), sistema_previo.obtener_estado());
    }

    // Fase B: fallo inyectado + verificación en segunda conexión + reintento exitoso en misma instancia.
    {
        let dir = DirectorioPrueba::nuevo(&format!("cv09_{etiqueta}_reintento"));
        let mut repo = RepositorioSqlite::abrir(dir.ruta()).expect("debe abrir BD");
        let (sistema_previo, borrador, cambios) = preparar_lote(&mut repo);

        repo.inyectar_fallo_escritura(punto);
        let err = repo
            .guardar(&cambios)
            .expect_err("el fallo inyectado debe abortar guardar()");
        assert_eq!(err.codigo, CodigoError::ErrorPersistencia);

        {
            let conn_aux = abrir_conexion_auxiliar(dir.ruta());
            assert_eq!(leer_ultima_secuencia_sql(&conn_aux), 1);
            assert_eq!(leer_turnos_sql(&conn_aux).len(), 1);
            assert_eq!(leer_ventanillas_sql(&conn_aux)[0].2, None);
        }

        let tras_fallo = repo
            .cargar()
            .expect("misma instancia debe seguir en autocommit y devolver estado previo");
        assert_eq!(tras_fallo.obtener_estado(), sistema_previo.obtener_estado());

        repo.guardar(&cambios)
            .expect("reintento en la misma instancia debe confirmar con éxito");

        let cargado_misma = repo
            .cargar()
            .expect("misma instancia debe leer el estado confirmado");
        assert_eq!(cargado_misma.ultima_secuencia(), 2);
        assert_eq!(cargado_misma.obtener_estado(), borrador.obtener_estado());
        drop(repo);

        let mut reabierto =
            RepositorioSqlite::abrir(dir.ruta()).expect("debe reabrir tras reintento");
        let cargado_reabierto = reabierto
            .cargar()
            .expect("reapertura debe conservar el reintento confirmado");
        assert_eq!(cargado_reabierto.ultima_secuencia(), 2);
        assert_eq!(
            cargado_reabierto.obtener_estado(),
            borrador.obtener_estado()
        );
    }
}

#[cfg(feature = "test-fault-injection")]
#[test]
fn cv09_fallo_despues_de_turnos_revierte_y_permite_reintento() {
    verificar_cv09_punto_fallo("despues_de_turnos", PuntoFalloEscritura::DespuesDeTurnos);
}

#[cfg(feature = "test-fault-injection")]
#[test]
fn cv09_fallo_despues_de_ventanillas_revierte_y_permite_reintento() {
    verificar_cv09_punto_fallo(
        "despues_de_ventanillas",
        PuntoFalloEscritura::DespuesDeVentanillas,
    );
}

#[cfg(feature = "test-fault-injection")]
#[test]
fn cv09_fallo_antes_de_commit_revierte_y_permite_reintento() {
    verificar_cv09_punto_fallo("antes_de_commit", PuntoFalloEscritura::AntesDeCommit);
}

fn crear_base_valida_en_modo_delete(ruta: &Path, turnos_extra: usize) {
    {
        let mut repo = RepositorioSqlite::abrir(ruta).expect("debe crear base inicial");
        let mut sistema = repo.cargar().expect("debe cargar inicial");
        if turnos_extra > 0 {
            let mut turnos = Vec::with_capacity(turnos_extra);
            for idx in 0..turnos_extra {
                let turno = sistema
                    .registrar_turno_en(instante_fijo(u32::try_from(idx + 1).expect("segundo")))
                    .expect("debe registrar turno");
                turnos.push(turno);
            }
            repo.guardar(&Cambios {
                turnos,
                ventanillas: vec![],
                ultima_secuencia: sistema.ultima_secuencia(),
            })
            .expect("debe persistir turnos semilla");
        }
    }
    let conn = abrir_conexion_auxiliar(ruta);
    let modo: String = conn
        .query_row("PRAGMA journal_mode = DELETE;", [], |r| r.get(0))
        .expect("debe cambiar a journal_mode=DELETE");
    assert_eq!(modo.to_ascii_lowercase(), "delete");
    conn.close().expect("debe cerrar conexión DELETE");
}

fn assert_sin_sidecars(dir: &DirectorioPrueba) {
    assert!(
        !dir.ruta_wal().exists(),
        "no debe crearse -wal en archivo corrupto no WAL"
    );
    assert!(
        !dir.ruta_shm().exists(),
        "no debe crearse -shm en archivo corrupto no WAL"
    );
    assert!(
        !PathBuf::from(format!("{}-journal", dir.ruta().display())).exists(),
        "no debe crearse -journal al rechazar archivo corrupto"
    );
}

#[test]
fn rd03_fisico_archivo_vacio_truncado_y_basura_conservan_bytes_sin_sidecars() {
    // 1. Archivo vacío (0 bytes).
    {
        let dir = DirectorioPrueba::nuevo("rd03_vacio");
        fs::create_dir_all(&dir.raiz).expect("debe crear directorio");
        fs::write(dir.ruta(), b"").expect("debe crear archivo vacío");

        let err = RepositorioSqlite::abrir(dir.ruta())
            .err()
            .expect("archivo vacío debe fallar en abrir");
        assert_eq!(err.codigo, CodigoError::BaseDatosCorrupta);
        assert_eq!(fs::read(dir.ruta()).expect("debe leer archivo"), b"");
        assert_sin_sidecars(&dir);
    }

    // 2. Archivo truncado (< 100 bytes, conserva firma mágica SQLite parcial).
    {
        let dir_origen = DirectorioPrueba::nuevo("rd03_origen_truncado_corto");
        crear_base_valida_en_modo_delete(dir_origen.ruta(), 2);
        let bytes_validos = fs::read(dir_origen.ruta()).expect("debe leer BD válida");
        assert!(bytes_validos.len() >= 4096);

        let dir = DirectorioPrueba::nuevo("rd03_truncado_64");
        fs::create_dir_all(&dir.raiz).expect("debe crear directorio");
        let truncado_64 = bytes_validos[..64].to_vec();
        fs::write(dir.ruta(), &truncado_64).expect("debe escribir archivo truncado < 100 bytes");

        let err = RepositorioSqlite::abrir(dir.ruta())
            .err()
            .expect("archivo truncado < 100 bytes debe fallar");
        assert_eq!(err.codigo, CodigoError::BaseDatosCorrupta);
        assert_eq!(
            fs::read(dir.ruta()).expect("debe leer archivo truncado"),
            truncado_64
        );
        assert_sin_sidecars(&dir);
    }

    // 3. Archivo truncado (>= 100 bytes pero cortado antes del tamaño completo de páginas).
    {
        let dir_origen = DirectorioPrueba::nuevo("rd03_origen_truncado_medio");
        crear_base_valida_en_modo_delete(dir_origen.ruta(), 4);
        let bytes_validos = fs::read(dir_origen.ruta()).expect("debe leer BD válida");
        assert!(bytes_validos.len() >= 8192);

        let dir = DirectorioPrueba::nuevo("rd03_truncado_2048");
        fs::create_dir_all(&dir.raiz).expect("debe crear directorio");
        let truncado_2048 = bytes_validos[..2048].to_vec();
        fs::write(dir.ruta(), &truncado_2048).expect("debe escribir archivo truncado >= 100 bytes");

        let err = RepositorioSqlite::abrir(dir.ruta())
            .err()
            .expect("archivo truncado en mitad de página debe fallar");
        assert_eq!(err.codigo, CodigoError::BaseDatosCorrupta);
        assert_eq!(
            fs::read(dir.ruta()).expect("debe leer archivo truncado 2048"),
            truncado_2048
        );
        assert_sin_sidecars(&dir);
    }

    // 4. Basura binaria no SQLite.
    {
        let dir = DirectorioPrueba::nuevo("rd03_basura");
        fs::create_dir_all(&dir.raiz).expect("debe crear directorio");
        let basura: Vec<u8> = (0..512)
            .map(|i| u8::try_from((i * 37 + 13) % 256).expect("byte"))
            .collect();
        fs::write(dir.ruta(), &basura).expect("debe escribir basura binaria");

        let err = RepositorioSqlite::abrir(dir.ruta())
            .err()
            .expect("archivo con basura binaria debe fallar");
        assert_eq!(err.codigo, CodigoError::BaseDatosCorrupta);
        assert_eq!(
            fs::read(dir.ruta()).expect("debe leer archivo basura"),
            basura
        );
        assert_sin_sidecars(&dir);
    }
}

#[test]
fn rd03_fisico_btree_corrupto_en_delete_y_wal_conserva_bytes() {
    // 1. B-tree corrupto en modo DELETE: cabecera intacta (primeros 100 bytes),
    //    página 2 dañada; falla con BaseDatosCorrupta, bytes idénticos y cero sidecars.
    {
        let dir = DirectorioPrueba::nuevo("rd03_btree_delete");
        crear_base_valida_en_modo_delete(dir.ruta(), 5);
        assert_sin_sidecars(&dir);

        let mut bytes = fs::read(dir.ruta()).expect("debe leer BD en DELETE");
        assert!(bytes.len() >= 8192, "la BD debe tener al menos 2 páginas");
        for byte in &mut bytes[4096..4160] {
            *byte = 0xFF;
        }
        fs::write(dir.ruta(), &bytes).expect("debe escribir BD DELETE con B-tree corrupto");

        let err = RepositorioSqlite::abrir(dir.ruta())
            .err()
            .expect("B-tree corrupto en modo DELETE debe fallar en abrir()");
        assert_eq!(err.codigo, CodigoError::BaseDatosCorrupta);
        assert_eq!(
            fs::read(dir.ruta()).expect("debe conservar bytes exactos"),
            bytes
        );
        assert_sin_sidecars(&dir);
    }

    // 2. B-tree corrupto en modo WAL: cabecera WAL intacta, página 2 dañada;
    //    falla con BaseDatosCorrupta y preserva bytes idénticos de sgt.db.
    {
        let dir = DirectorioPrueba::nuevo("rd03_btree_wal");
        {
            let mut repo = RepositorioSqlite::abrir(dir.ruta()).expect("debe crear BD WAL");
            let mut sistema = repo.cargar().expect("debe cargar inicial");
            let t1 = sistema
                .registrar_turno_en(instante_fijo(1))
                .expect("T-0001");
            repo.guardar(&Cambios {
                turnos: vec![t1],
                ventanillas: vec![],
                ultima_secuencia: 1,
            })
            .expect("debe guardar T-0001");
        }

        let mut bytes = fs::read(dir.ruta()).expect("debe leer BD en WAL");
        assert!(
            bytes.len() >= 8192,
            "la BD WAL debe tener al menos 2 páginas"
        );
        assert_eq!(bytes[18], 2, "cabecera debe indicar modo WAL");
        for byte in &mut bytes[4096..4160] {
            *byte = 0xFF;
        }
        fs::write(dir.ruta(), &bytes).expect("debe escribir BD WAL con B-tree corrupto");

        let err = RepositorioSqlite::abrir(dir.ruta())
            .err()
            .expect("B-tree corrupto en modo WAL debe fallar en abrir()");
        assert_eq!(err.codigo, CodigoError::BaseDatosCorrupta);
        assert_eq!(
            fs::read(dir.ruta()).expect("debe conservar bytes exactos de BD WAL corrupta"),
            bytes
        );
    }
}

#[test]
fn rd03_fisico_violacion_fk_devuelve_base_corrupta_y_conserva_bytes_sin_sidecars() {
    let dir = DirectorioPrueba::nuevo("rd03_fk_violacion");
    crear_base_valida_en_modo_delete(dir.ruta(), 0);

    {
        let conn = Connection::open_with_flags(
            dir.ruta(),
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .expect("debe abrir BD en modo DELETE");
        conn.execute_batch(
            "PRAGMA foreign_keys = OFF;
             UPDATE secuencias SET ultimo_valor = 1 WHERE clave = 'turno';
             INSERT INTO turnos (id, secuencia, fecha_hora_registro, estado, ventanilla_asignada)
             VALUES ('T-0001', 1, '2026-11-26T15:00:01.000Z', 'ATENDIDO', 'V99');",
        )
        .expect("debe inyectar violación FK con foreign_keys = OFF");
        conn.close().expect("debe cerrar conexión tras inyectar FK");
    }

    assert_sin_sidecars(&dir);
    let bytes_antes = fs::read(dir.ruta()).expect("debe leer bytes antes de abrir");

    let err = RepositorioSqlite::abrir(dir.ruta())
        .err()
        .expect("violación de clave foránea debe rechazarse en abrir()");
    assert_eq!(err.codigo, CodigoError::BaseDatosCorrupta);
    assert_eq!(
        fs::read(dir.ruta()).expect("debe conservar bytes idénticos tras rechazo FK"),
        bytes_antes
    );
    assert_sin_sidecars(&dir);
}

fn verificar_rd03_logico(
    etiqueta: &str,
    sembrar: impl FnOnce(&mut RepositorioSqlite),
    corromper: impl FnOnce(&Connection),
    reparar: impl FnOnce(&Connection),
) {
    let dir = DirectorioPrueba::nuevo(etiqueta);
    {
        let mut repo = RepositorioSqlite::abrir(dir.ruta()).expect("debe abrir BD inicial");
        sembrar(&mut repo);
    }

    let conn_aux = abrir_conexion_auxiliar(dir.ruta());
    corromper(&conn_aux);
    let snapshot_corrupto = capturar_snapshot_sql(&conn_aux);

    let mut repo = RepositorioSqlite::abrir(dir.ruta())
        .expect("esquema físicamente íntegro debe abrir para validar lógica en cargar()");

    let err1 = repo
        .cargar()
        .expect_err("primera llamada a cargar() debe rechazar corrupción lógica");
    assert_eq!(err1.codigo, CodigoError::BaseDatosCorrupta);

    // Segunda llamada en la misma instancia: si no hubiera hecho rollback limpio
    // manteniendo autocommit=true, devolvería ErrorPersistencia por estado incierto.
    let err2 = repo
        .cargar()
        .expect_err("segunda llamada debe seguir devolviendo BaseDatosCorrupta (autocommit)");
    assert_eq!(err2.codigo, CodigoError::BaseDatosCorrupta);

    assert_eq!(
        capturar_snapshot_sql(&conn_aux),
        snapshot_corrupto,
        "cargar() no debe alterar ni reparar silenciosamente ninguna fila o tabla"
    );

    // Al reparar el defecto externamente, la misma instancia debe cargar con éxito,
    // confirmando que permaneció en autocommit y completamente reutilizable.
    reparar(&conn_aux);
    drop(conn_aux);

    repo.cargar()
        .expect("tras reparar la inconsistencia lógica, la misma instancia debe reutilizarse");
}

#[test]
fn rd03_logico_schema_migrations_vacia_desconocida_y_faltante() {
    // 1. Tabla schema_migrations vacía.
    verificar_rd03_logico(
        "rd03_logico_migraciones_vacia",
        |_| {},
        |conn| {
            conn.execute("DELETE FROM schema_migrations", [])
                .expect("debe vaciar schema_migrations");
        },
        |conn| {
            conn.execute(
                "INSERT INTO schema_migrations (version, nombre) VALUES (1, '001_initial')",
                [],
            )
            .expect("debe restaurar migración 1");
        },
    );

    // 2. Versión desconocida en schema_migrations.
    verificar_rd03_logico(
        "rd03_logico_migraciones_desconocida",
        |_| {},
        |conn| {
            conn.execute(
                "UPDATE schema_migrations SET version = 2, nombre = '002_ajena' WHERE version = 1",
                [],
            )
            .expect("debe reemplazar por versión desconocida");
        },
        |conn| {
            conn.execute(
                "UPDATE schema_migrations SET version = 1, nombre = '001_initial' WHERE version = 2",
                [],
            )
            .expect("debe restaurar versión 1");
        },
    );

    // 3. Versión extra adicional en schema_migrations ([1, 2]).
    verificar_rd03_logico(
        "rd03_logico_migraciones_extra",
        |_| {},
        |conn| {
            conn.execute(
                "INSERT INTO schema_migrations (version, nombre) VALUES (2, '002_extra')",
                [],
            )
            .expect("debe insertar migración extra");
        },
        |conn| {
            conn.execute("DELETE FROM schema_migrations WHERE version = 2", [])
                .expect("debe eliminar migración extra");
        },
    );

    // 4. Tabla schema_migrations faltante.
    verificar_rd03_logico(
        "rd03_logico_migraciones_faltante",
        |_| {},
        |conn| {
            conn.execute("DROP TABLE schema_migrations", [])
                .expect("debe eliminar tabla schema_migrations");
        },
        |conn| {
            conn.execute_batch(
                "CREATE TABLE schema_migrations (
                    version     INTEGER PRIMARY KEY CHECK (version > 0),
                    nombre      TEXT    NOT NULL UNIQUE CHECK (length(trim(nombre)) > 0),
                    aplicada_en TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
                );
                INSERT INTO schema_migrations (version, nombre) VALUES (1, '001_initial');",
            )
            .expect("debe recrear schema_migrations");
        },
    );
}

#[test]
fn rd03_logico_secuencia_ausente_y_ninguna_ventanilla_activa() {
    // 1. Fila 'turno' ausente en secuencias.
    verificar_rd03_logico(
        "rd03_logico_secuencia_fila_ausente",
        |_| {},
        |conn| {
            conn.execute("DELETE FROM secuencias WHERE clave = 'turno'", [])
                .expect("debe eliminar fila de secuencia");
        },
        |conn| {
            conn.execute(
                "INSERT INTO secuencias (clave, ultimo_valor) VALUES ('turno', 0)",
                [],
            )
            .expect("debe restaurar fila de secuencia");
        },
    );

    // 2. Tabla secuencias ausente.
    verificar_rd03_logico(
        "rd03_logico_secuencia_tabla_ausente",
        |_| {},
        |conn| {
            conn.execute("DROP TABLE secuencias", [])
                .expect("debe eliminar tabla secuencias");
        },
        |conn| {
            conn.execute_batch(
                "CREATE TABLE secuencias (
                    clave        TEXT    PRIMARY KEY CHECK (length(trim(clave)) > 0),
                    ultimo_valor INTEGER NOT NULL DEFAULT 0 CHECK (ultimo_valor >= 0)
                );
                INSERT INTO secuencias (clave, ultimo_valor) VALUES ('turno', 0);",
            )
            .expect("debe recrear tabla secuencias");
        },
    );

    // 3. Ninguna ventanilla activa (todas con habilitada = 0).
    verificar_rd03_logico(
        "rd03_logico_ninguna_ventanilla_activa",
        |_| {},
        |conn| {
            conn.execute_batch(
                "DROP TRIGGER trg_ventanillas_minimo_una_activa;
                 UPDATE ventanillas SET habilitada = 0;",
            )
            .expect("debe deshabilitar todas las ventanillas");
        },
        |conn| {
            conn.execute("UPDATE ventanillas SET habilitada = 1 WHERE id = 'V01'", [])
                .expect("debe habilitar V01");
        },
    );
}

#[test]
fn rd03_logico_referencias_invalidas_y_atencion_sin_reflejo() {
    // 1. Ventanilla apunta a turno en estado EN_ESPERA.
    verificar_rd03_logico(
        "rd03_logico_ref_en_espera",
        |repo| {
            let mut sistema = repo.cargar().expect("cargar inicial");
            let t1 = sistema
                .registrar_turno_en(instante_fijo(1))
                .expect("T-0001");
            repo.guardar(&Cambios {
                turnos: vec![t1],
                ventanillas: vec![],
                ultima_secuencia: 1,
            })
            .expect("guardar T-0001 EN_ESPERA");
        },
        |conn| {
            conn.execute(
                "UPDATE ventanillas SET turno_actual_id = 'T-0001' WHERE id = 'V01'",
                [],
            )
            .expect("debe apuntar V01 a turno EN_ESPERA");
        },
        |conn| {
            conn.execute(
                "UPDATE ventanillas SET turno_actual_id = NULL WHERE id = 'V01'",
                [],
            )
            .expect("debe limpiar referencia inválida");
        },
    );

    // 2. Ventanilla apunta a turno en estado ATENDIDO.
    verificar_rd03_logico(
        "rd03_logico_ref_atendido",
        |repo| {
            let mut sistema = repo.cargar().expect("cargar inicial");
            let t1 = sistema
                .registrar_turno_en(instante_fijo(1))
                .expect("T-0001");
            repo.guardar(&Cambios {
                turnos: vec![t1],
                ventanillas: vec![],
                ultima_secuencia: 1,
            })
            .expect("guardar T-0001");
            let t1_asignado = sistema
                .asignar_siguiente(VentanillaId::V01)
                .expect("asignar T-0001");
            repo.guardar(&Cambios {
                turnos: vec![t1_asignado],
                ventanillas: vec![sistema.ventanilla(VentanillaId::V01).clone()],
                ultima_secuencia: 1,
            })
            .expect("guardar T-0001 EN_ATENCION");
            let t1_atendido = sistema
                .finalizar_atencion(VentanillaId::V01)
                .expect("finalizar T-0001");
            repo.guardar(&Cambios {
                turnos: vec![t1_atendido],
                ventanillas: vec![sistema.ventanilla(VentanillaId::V01).clone()],
                ultima_secuencia: 1,
            })
            .expect("guardar T-0001 ATENDIDO");
        },
        |conn| {
            conn.execute(
                "UPDATE ventanillas SET turno_actual_id = 'T-0001' WHERE id = 'V01'",
                [],
            )
            .expect("debe apuntar V01 a turno ATENDIDO");
        },
        |conn| {
            conn.execute(
                "UPDATE ventanillas SET turno_actual_id = NULL WHERE id = 'V01'",
                [],
            )
            .expect("debe limpiar referencia inválida");
        },
    );

    // 3. Turno EN_ATENCION sin reflejo en ventanillas.turno_actual_id.
    verificar_rd03_logico(
        "rd03_logico_atencion_sin_reflejo",
        |repo| {
            let mut sistema = repo.cargar().expect("cargar inicial");
            let t1 = sistema
                .registrar_turno_en(instante_fijo(1))
                .expect("T-0001");
            repo.guardar(&Cambios {
                turnos: vec![t1],
                ventanillas: vec![],
                ultima_secuencia: 1,
            })
            .expect("guardar T-0001");
            let t1_asignado = sistema
                .asignar_siguiente(VentanillaId::V01)
                .expect("asignar T-0001");
            repo.guardar(&Cambios {
                turnos: vec![t1_asignado],
                ventanillas: vec![sistema.ventanilla(VentanillaId::V01).clone()],
                ultima_secuencia: 1,
            })
            .expect("guardar T-0001 EN_ATENCION");
        },
        |conn| {
            conn.execute(
                "UPDATE ventanillas SET turno_actual_id = NULL WHERE id = 'V01'",
                [],
            )
            .expect("debe eliminar reflejo en V01");
        },
        |conn| {
            conn.execute(
                "UPDATE ventanillas SET turno_actual_id = 'T-0001' WHERE id = 'V01'",
                [],
            )
            .expect("debe restaurar reflejo en V01");
        },
    );
}

// ===========================================================================
// VFS observador para verificar que no se abran/creen sidecars en disco (C)
// ===========================================================================

#[derive(Default)]
struct RegistroVfsDir {
    aperturas: Vec<(String, c_int)>,
    accesos: Vec<(String, c_int)>,
    borrados: Vec<String>,
    shm_maps: Vec<String>,
}

#[derive(Clone)]
struct ConfigCorteWal {
    armado: Arc<AtomicBool>,
    corte_aplicado: Arc<AtomicBool>,
    umbral_offset: Arc<AtomicI64>,
    bytes_corte: c_int,
}

#[derive(Clone)]
struct ReglaObservadorVfs {
    id: u64,
    subcadena_ruta: String,
    registro: Arc<Mutex<RegistroVfsDir>>,
    corte_wal: Option<ConfigCorteWal>,
}

#[repr(C)]
#[derive(Copy, Clone)]
struct ArchivoObsVfsExtra {
    orig_methods: *const ffi::sqlite3_io_methods,
    id_regla: u64,
    ruta_ptr: *mut String,
}

struct EstadoObsVfs {
    _base_vfs: usize,
    base_sz_alineado: usize,
    _vfs_box: usize,
}

static ESTADO_OBS_VFS: OnceLock<EstadoObsVfs> = OnceLock::new();
static REGLAS_OBS_VFS: Mutex<Vec<ReglaObservadorVfs>> = Mutex::new(Vec::new());
static SIGUIENTE_REGLA_OBS_VFS: AtomicU64 = AtomicU64::new(1);

const fn align_up(valor: usize, align: usize) -> usize {
    (valor + align - 1) & !(align - 1)
}

unsafe fn base_obs_vfs_desde(p_vfs: *mut ffi::sqlite3_vfs) -> *mut ffi::sqlite3_vfs {
    unsafe { (*p_vfs).pAppData.cast::<ffi::sqlite3_vfs>() }
}

unsafe fn extra_obs_archivo(p_file: *mut ffi::sqlite3_file) -> *mut ArchivoObsVfsExtra {
    let offset = ESTADO_OBS_VFS
        .get()
        .expect("VFS observador inicializado")
        .base_sz_alineado;
    unsafe { p_file.cast::<u8>().add(offset).cast::<ArchivoObsVfsExtra>() }
}

unsafe fn orig_obs_methods_de(p_file: *mut ffi::sqlite3_file) -> *const ffi::sqlite3_io_methods {
    unsafe { (*extra_obs_archivo(p_file)).orig_methods }
}

fn buscar_obs_regla_por_ruta(ruta: &str) -> Option<ReglaObservadorVfs> {
    REGLAS_OBS_VFS
        .lock()
        .expect("mutex de reglas observador VFS")
        .iter()
        .find(|r| ruta.contains(&r.subcadena_ruta))
        .cloned()
}

fn buscar_obs_regla_por_id(id: u64) -> Option<ReglaObservadorVfs> {
    REGLAS_OBS_VFS
        .lock()
        .expect("mutex de reglas observador VFS")
        .iter()
        .find(|r| r.id == id)
        .cloned()
}

unsafe extern "C" fn obs_vfs_open(
    p_vfs: *mut ffi::sqlite3_vfs,
    z_name: *const c_char,
    p_file: *mut ffi::sqlite3_file,
    flags: c_int,
    p_out_flags: *mut c_int,
) -> c_int {
    let ruta_match = if !z_name.is_null() {
        let ruta = unsafe { CStr::from_ptr(z_name) }
            .to_string_lossy()
            .into_owned();
        if let Some(regla) = buscar_obs_regla_por_ruta(&ruta) {
            regla
                .registro
                .lock()
                .expect("registro VFS")
                .aperturas
                .push((ruta.clone(), flags));
            Some((regla, ruta))
        } else {
            None
        }
    } else {
        None
    };

    let base = unsafe { base_obs_vfs_desde(p_vfs) };
    let rc =
        unsafe { ((*base).xOpen.expect("base xOpen"))(base, z_name, p_file, flags, p_out_flags) };
    if rc != ffi::SQLITE_OK {
        return rc;
    }
    let Some((regla, ruta)) = ruta_match else {
        return rc;
    };
    let orig_methods = unsafe { (*p_file).pMethods };
    if orig_methods.is_null() {
        return rc;
    }
    unsafe {
        let extra = extra_obs_archivo(p_file);
        extra.write(ArchivoObsVfsExtra {
            orig_methods,
            id_regla: regla.id,
            ruta_ptr: Box::into_raw(Box::new(ruta)),
        });
        (*p_file).pMethods = if (*orig_methods).iVersion >= 3 && (*orig_methods).xFetch.is_some() {
            &raw const OBS_WRAPPER_IO_V3
        } else {
            &raw const OBS_WRAPPER_IO_V2
        };
    }
    rc
}

unsafe extern "C" fn obs_vfs_delete(
    p_vfs: *mut ffi::sqlite3_vfs,
    z_name: *const c_char,
    sync_dir: c_int,
) -> c_int {
    if !z_name.is_null() {
        let ruta = unsafe { CStr::from_ptr(z_name) }
            .to_string_lossy()
            .into_owned();
        if let Some(regla) = buscar_obs_regla_por_ruta(&ruta) {
            regla
                .registro
                .lock()
                .expect("registro VFS")
                .borrados
                .push(ruta);
        }
    }
    let base = unsafe { base_obs_vfs_desde(p_vfs) };
    unsafe { ((*base).xDelete.expect("base xDelete"))(base, z_name, sync_dir) }
}

unsafe extern "C" fn obs_vfs_access(
    p_vfs: *mut ffi::sqlite3_vfs,
    z_name: *const c_char,
    flags: c_int,
    p_res_out: *mut c_int,
) -> c_int {
    if !z_name.is_null() {
        let ruta = unsafe { CStr::from_ptr(z_name) }
            .to_string_lossy()
            .into_owned();
        if let Some(regla) = buscar_obs_regla_por_ruta(&ruta) {
            regla
                .registro
                .lock()
                .expect("registro VFS")
                .accesos
                .push((ruta, flags));
        }
    }
    let base = unsafe { base_obs_vfs_desde(p_vfs) };
    unsafe { ((*base).xAccess.expect("base xAccess"))(base, z_name, flags, p_res_out) }
}

unsafe extern "C" fn obs_vfs_full_pathname(
    p_vfs: *mut ffi::sqlite3_vfs,
    z_name: *const c_char,
    n_out: c_int,
    z_out: *mut c_char,
) -> c_int {
    let base = unsafe { base_obs_vfs_desde(p_vfs) };
    unsafe { ((*base).xFullPathname.expect("base xFullPathname"))(base, z_name, n_out, z_out) }
}

unsafe extern "C" fn obs_vfs_dl_open(
    p_vfs: *mut ffi::sqlite3_vfs,
    z_filename: *const c_char,
) -> *mut c_void {
    let base = unsafe { base_obs_vfs_desde(p_vfs) };
    unsafe { ((*base).xDlOpen.expect("base xDlOpen"))(base, z_filename) }
}

unsafe extern "C" fn obs_vfs_dl_error(
    p_vfs: *mut ffi::sqlite3_vfs,
    n_byte: c_int,
    z_err_msg: *mut c_char,
) {
    let base = unsafe { base_obs_vfs_desde(p_vfs) };
    unsafe { ((*base).xDlError.expect("base xDlError"))(base, n_byte, z_err_msg) }
}

unsafe extern "C" fn obs_vfs_dl_sym(
    p_vfs: *mut ffi::sqlite3_vfs,
    p_handle: *mut c_void,
    z_symbol: *const c_char,
) -> Option<unsafe extern "C" fn(*mut ffi::sqlite3_vfs, *mut c_void, *const c_char)> {
    let base = unsafe { base_obs_vfs_desde(p_vfs) };
    unsafe { ((*base).xDlSym.expect("base xDlSym"))(base, p_handle, z_symbol) }
}

unsafe extern "C" fn obs_vfs_dl_close(p_vfs: *mut ffi::sqlite3_vfs, p_handle: *mut c_void) {
    let base = unsafe { base_obs_vfs_desde(p_vfs) };
    unsafe { ((*base).xDlClose.expect("base xDlClose"))(base, p_handle) }
}

unsafe extern "C" fn obs_vfs_randomness(
    p_vfs: *mut ffi::sqlite3_vfs,
    n_byte: c_int,
    z_out: *mut c_char,
) -> c_int {
    let base = unsafe { base_obs_vfs_desde(p_vfs) };
    unsafe { ((*base).xRandomness.expect("base xRandomness"))(base, n_byte, z_out) }
}

unsafe extern "C" fn obs_vfs_sleep(p_vfs: *mut ffi::sqlite3_vfs, microseconds: c_int) -> c_int {
    let base = unsafe { base_obs_vfs_desde(p_vfs) };
    unsafe { ((*base).xSleep.expect("base xSleep"))(base, microseconds) }
}

unsafe extern "C" fn obs_vfs_current_time(p_vfs: *mut ffi::sqlite3_vfs, pr_now: *mut f64) -> c_int {
    let base = unsafe { base_obs_vfs_desde(p_vfs) };
    unsafe { ((*base).xCurrentTime.expect("base xCurrentTime"))(base, pr_now) }
}

unsafe extern "C" fn obs_vfs_get_last_error(
    p_vfs: *mut ffi::sqlite3_vfs,
    n_buf: c_int,
    z_buf: *mut c_char,
) -> c_int {
    let base = unsafe { base_obs_vfs_desde(p_vfs) };
    unsafe { ((*base).xGetLastError.expect("base xGetLastError"))(base, n_buf, z_buf) }
}

unsafe extern "C" fn obs_vfs_current_time_int64(
    p_vfs: *mut ffi::sqlite3_vfs,
    pi_now: *mut ffi::sqlite3_int64,
) -> c_int {
    let base = unsafe { base_obs_vfs_desde(p_vfs) };
    unsafe { ((*base).xCurrentTimeInt64.expect("base xCurrentTimeInt64"))(base, pi_now) }
}

unsafe extern "C" fn obs_vfs_set_system_call(
    p_vfs: *mut ffi::sqlite3_vfs,
    z_name: *const c_char,
    p_new_func: ffi::sqlite3_syscall_ptr,
) -> c_int {
    let base = unsafe { base_obs_vfs_desde(p_vfs) };
    unsafe { ((*base).xSetSystemCall.expect("base xSetSystemCall"))(base, z_name, p_new_func) }
}

unsafe extern "C" fn obs_vfs_get_system_call(
    p_vfs: *mut ffi::sqlite3_vfs,
    z_name: *const c_char,
) -> ffi::sqlite3_syscall_ptr {
    let base = unsafe { base_obs_vfs_desde(p_vfs) };
    unsafe { ((*base).xGetSystemCall.expect("base xGetSystemCall"))(base, z_name) }
}

unsafe extern "C" fn obs_vfs_next_system_call(
    p_vfs: *mut ffi::sqlite3_vfs,
    z_name: *const c_char,
) -> *const c_char {
    let base = unsafe { base_obs_vfs_desde(p_vfs) };
    unsafe { ((*base).xNextSystemCall.expect("base xNextSystemCall"))(base, z_name) }
}

unsafe extern "C" fn obs_io_close(p_file: *mut ffi::sqlite3_file) -> c_int {
    let extra = unsafe { *extra_obs_archivo(p_file) };
    unsafe {
        (*p_file).pMethods = extra.orig_methods;
    }
    let rc = unsafe { ((*extra.orig_methods).xClose.expect("orig xClose"))(p_file) };
    if !extra.ruta_ptr.is_null() {
        unsafe {
            drop(Box::from_raw(extra.ruta_ptr));
        }
    }
    rc
}

unsafe extern "C" fn obs_io_read(
    p_file: *mut ffi::sqlite3_file,
    z_buf: *mut c_void,
    i_amt: c_int,
    i_ofst: ffi::sqlite3_int64,
) -> c_int {
    let orig = unsafe { orig_obs_methods_de(p_file) };
    unsafe { ((*orig).xRead.expect("orig xRead"))(p_file, z_buf, i_amt, i_ofst) }
}

unsafe extern "C" fn obs_io_write(
    p_file: *mut ffi::sqlite3_file,
    z_buf: *const c_void,
    i_amt: c_int,
    i_ofst: ffi::sqlite3_int64,
) -> c_int {
    let extra = unsafe { *extra_obs_archivo(p_file) };
    if let Some(regla) = buscar_obs_regla_por_id(extra.id_regla)
        && let Some(corte) = regla.corte_wal
        && corte.armado.load(Ordering::SeqCst)
    {
        let ruta = unsafe { &*extra.ruta_ptr };
        if ruta.ends_with("-wal") && !ruta.contains(".tmp-") {
            let umbral = corte.umbral_offset.load(Ordering::SeqCst);
            if i_ofst >= umbral {
                if corte.bytes_corte > 0 && !corte.corte_aplicado.swap(true, Ordering::SeqCst) {
                    let parcial = i_amt.min(corte.bytes_corte);
                    let rc = unsafe {
                        ((*extra.orig_methods).xWrite.expect("orig xWrite"))(
                            p_file, z_buf, parcial, i_ofst,
                        )
                    };
                    assert_eq!(rc, ffi::SQLITE_OK, "escritura parcial de corte VFS");
                } else {
                    corte.corte_aplicado.store(true, Ordering::SeqCst);
                }
                return ffi::SQLITE_IOERR_WRITE;
            }
        }
    }
    unsafe { ((*extra.orig_methods).xWrite.expect("orig xWrite"))(p_file, z_buf, i_amt, i_ofst) }
}

unsafe extern "C" fn obs_io_truncate(
    p_file: *mut ffi::sqlite3_file,
    size: ffi::sqlite3_int64,
) -> c_int {
    let orig = unsafe { orig_obs_methods_de(p_file) };
    unsafe { ((*orig).xTruncate.expect("orig xTruncate"))(p_file, size) }
}

unsafe extern "C" fn obs_io_sync(p_file: *mut ffi::sqlite3_file, flags: c_int) -> c_int {
    let orig = unsafe { orig_obs_methods_de(p_file) };
    unsafe { ((*orig).xSync.expect("orig xSync"))(p_file, flags) }
}

unsafe extern "C" fn obs_io_file_size(
    p_file: *mut ffi::sqlite3_file,
    p_size: *mut ffi::sqlite3_int64,
) -> c_int {
    let orig = unsafe { orig_obs_methods_de(p_file) };
    unsafe { ((*orig).xFileSize.expect("orig xFileSize"))(p_file, p_size) }
}

unsafe extern "C" fn obs_io_lock(p_file: *mut ffi::sqlite3_file, level: c_int) -> c_int {
    let orig = unsafe { orig_obs_methods_de(p_file) };
    unsafe { ((*orig).xLock.expect("orig xLock"))(p_file, level) }
}

unsafe extern "C" fn obs_io_unlock(p_file: *mut ffi::sqlite3_file, level: c_int) -> c_int {
    let orig = unsafe { orig_obs_methods_de(p_file) };
    unsafe { ((*orig).xUnlock.expect("orig xUnlock"))(p_file, level) }
}

unsafe extern "C" fn obs_io_check_reserved_lock(
    p_file: *mut ffi::sqlite3_file,
    p_res_out: *mut c_int,
) -> c_int {
    let orig = unsafe { orig_obs_methods_de(p_file) };
    unsafe { ((*orig).xCheckReservedLock.expect("orig xCheckReservedLock"))(p_file, p_res_out) }
}

unsafe extern "C" fn obs_io_file_control(
    p_file: *mut ffi::sqlite3_file,
    op: c_int,
    p_arg: *mut c_void,
) -> c_int {
    let orig = unsafe { orig_obs_methods_de(p_file) };
    unsafe { ((*orig).xFileControl.expect("orig xFileControl"))(p_file, op, p_arg) }
}

unsafe extern "C" fn obs_io_sector_size(p_file: *mut ffi::sqlite3_file) -> c_int {
    let orig = unsafe { orig_obs_methods_de(p_file) };
    unsafe { ((*orig).xSectorSize.expect("orig xSectorSize"))(p_file) }
}

unsafe extern "C" fn obs_io_device_characteristics(p_file: *mut ffi::sqlite3_file) -> c_int {
    let orig = unsafe { orig_obs_methods_de(p_file) };
    unsafe {
        ((*orig)
            .xDeviceCharacteristics
            .expect("orig xDeviceCharacteristics"))(p_file)
    }
}

unsafe extern "C" fn obs_io_shm_map(
    p_file: *mut ffi::sqlite3_file,
    i_pg: c_int,
    pgsz: c_int,
    b_extend: c_int,
    pp: *mut *mut c_void,
) -> c_int {
    let extra = unsafe { *extra_obs_archivo(p_file) };
    if let Some(regla) = buscar_obs_regla_por_id(extra.id_regla) {
        let ruta = unsafe { &*extra.ruta_ptr }.clone();
        regla
            .registro
            .lock()
            .expect("registro VFS")
            .shm_maps
            .push(ruta);
    }
    unsafe {
        ((*extra.orig_methods).xShmMap.expect("orig xShmMap"))(p_file, i_pg, pgsz, b_extend, pp)
    }
}

unsafe extern "C" fn obs_io_shm_lock(
    p_file: *mut ffi::sqlite3_file,
    offset: c_int,
    n: c_int,
    flags: c_int,
) -> c_int {
    let orig = unsafe { orig_obs_methods_de(p_file) };
    unsafe { ((*orig).xShmLock.expect("orig xShmLock"))(p_file, offset, n, flags) }
}

unsafe extern "C" fn obs_io_shm_barrier(p_file: *mut ffi::sqlite3_file) {
    let orig = unsafe { orig_obs_methods_de(p_file) };
    unsafe { ((*orig).xShmBarrier.expect("orig xShmBarrier"))(p_file) }
}

unsafe extern "C" fn obs_io_shm_unmap(p_file: *mut ffi::sqlite3_file, delete_flag: c_int) -> c_int {
    let orig = unsafe { orig_obs_methods_de(p_file) };
    unsafe { ((*orig).xShmUnmap.expect("orig xShmUnmap"))(p_file, delete_flag) }
}

unsafe extern "C" fn obs_io_fetch(
    p_file: *mut ffi::sqlite3_file,
    i_ofst: ffi::sqlite3_int64,
    i_amt: c_int,
    pp: *mut *mut c_void,
) -> c_int {
    let orig = unsafe { orig_obs_methods_de(p_file) };
    unsafe { ((*orig).xFetch.expect("orig xFetch"))(p_file, i_ofst, i_amt, pp) }
}

unsafe extern "C" fn obs_io_unfetch(
    p_file: *mut ffi::sqlite3_file,
    i_ofst: ffi::sqlite3_int64,
    p: *mut c_void,
) -> c_int {
    let orig = unsafe { orig_obs_methods_de(p_file) };
    unsafe { ((*orig).xUnfetch.expect("orig xUnfetch"))(p_file, i_ofst, p) }
}

static OBS_WRAPPER_IO_V2: ffi::sqlite3_io_methods = ffi::sqlite3_io_methods {
    iVersion: 2,
    xClose: Some(obs_io_close),
    xRead: Some(obs_io_read),
    xWrite: Some(obs_io_write),
    xTruncate: Some(obs_io_truncate),
    xSync: Some(obs_io_sync),
    xFileSize: Some(obs_io_file_size),
    xLock: Some(obs_io_lock),
    xUnlock: Some(obs_io_unlock),
    xCheckReservedLock: Some(obs_io_check_reserved_lock),
    xFileControl: Some(obs_io_file_control),
    xSectorSize: Some(obs_io_sector_size),
    xDeviceCharacteristics: Some(obs_io_device_characteristics),
    xShmMap: Some(obs_io_shm_map),
    xShmLock: Some(obs_io_shm_lock),
    xShmBarrier: Some(obs_io_shm_barrier),
    xShmUnmap: Some(obs_io_shm_unmap),
    xFetch: None,
    xUnfetch: None,
};

static OBS_WRAPPER_IO_V3: ffi::sqlite3_io_methods = ffi::sqlite3_io_methods {
    iVersion: 3,
    xClose: Some(obs_io_close),
    xRead: Some(obs_io_read),
    xWrite: Some(obs_io_write),
    xTruncate: Some(obs_io_truncate),
    xSync: Some(obs_io_sync),
    xFileSize: Some(obs_io_file_size),
    xLock: Some(obs_io_lock),
    xUnlock: Some(obs_io_unlock),
    xCheckReservedLock: Some(obs_io_check_reserved_lock),
    xFileControl: Some(obs_io_file_control),
    xSectorSize: Some(obs_io_sector_size),
    xDeviceCharacteristics: Some(obs_io_device_characteristics),
    xShmMap: Some(obs_io_shm_map),
    xShmLock: Some(obs_io_shm_lock),
    xShmBarrier: Some(obs_io_shm_barrier),
    xShmUnmap: Some(obs_io_shm_unmap),
    xFetch: Some(obs_io_fetch),
    xUnfetch: Some(obs_io_unfetch),
};

fn asegurar_obs_vfs_registrado() {
    let _ = ESTADO_OBS_VFS.get_or_init(|| unsafe {
        let base = ffi::sqlite3_vfs_find(ptr::null());
        assert!(!base.is_null(), "debe existir un VFS SQLite por defecto");
        let base_sz = usize::try_from((*base).szOsFile).expect("szOsFile positivo");
        let base_sz_alineado = align_up(base_sz, align_of::<ArchivoObsVfsExtra>());
        let sz_total = c_int::try_from(base_sz_alineado + size_of::<ArchivoObsVfsExtra>())
            .expect("szOsFile total cabe en c_int");

        let vfs = Box::new(ffi::sqlite3_vfs {
            iVersion: (*base).iVersion.min(3),
            szOsFile: sz_total,
            mxPathname: (*base).mxPathname,
            pNext: ptr::null_mut(),
            zName: c"sgt_resiliencia_vfs".as_ptr(),
            pAppData: base.cast::<c_void>(),
            xOpen: Some(obs_vfs_open),
            xDelete: Some(obs_vfs_delete),
            xAccess: Some(obs_vfs_access),
            xFullPathname: Some(obs_vfs_full_pathname),
            xDlOpen: (*base).xDlOpen.map(|_| obs_vfs_dl_open as _),
            xDlError: (*base).xDlError.map(|_| obs_vfs_dl_error as _),
            xDlSym: (*base).xDlSym.map(|_| obs_vfs_dl_sym as _),
            xDlClose: (*base).xDlClose.map(|_| obs_vfs_dl_close as _),
            xRandomness: (*base).xRandomness.map(|_| obs_vfs_randomness as _),
            xSleep: (*base).xSleep.map(|_| obs_vfs_sleep as _),
            xCurrentTime: (*base).xCurrentTime.map(|_| obs_vfs_current_time as _),
            xGetLastError: (*base).xGetLastError.map(|_| obs_vfs_get_last_error as _),
            xCurrentTimeInt64: (*base)
                .xCurrentTimeInt64
                .map(|_| obs_vfs_current_time_int64 as _),
            xSetSystemCall: (*base).xSetSystemCall.map(|_| obs_vfs_set_system_call as _),
            xGetSystemCall: (*base).xGetSystemCall.map(|_| obs_vfs_get_system_call as _),
            xNextSystemCall: (*base)
                .xNextSystemCall
                .map(|_| obs_vfs_next_system_call as _),
        });
        let vfs_ptr = Box::into_raw(vfs);
        let rc = ffi::sqlite3_vfs_register(vfs_ptr, 1);
        assert_eq!(rc, ffi::SQLITE_OK, "debe registrar VFS observador");
        EstadoObsVfs {
            _base_vfs: base as usize,
            base_sz_alineado,
            _vfs_box: vfs_ptr as usize,
        }
    });
}

struct GuardiaObservadorVfs {
    id: u64,
}

impl GuardiaObservadorVfs {
    fn instalar(dir: &DirectorioPrueba) -> (Self, Arc<Mutex<RegistroVfsDir>>) {
        Self::instalar_interno(&dir.raiz, None)
    }

    fn instalar_con_corte_wal(
        raiz_dir: &Path,
        corte: ConfigCorteWal,
    ) -> (Self, Arc<Mutex<RegistroVfsDir>>) {
        Self::instalar_interno(raiz_dir, Some(corte))
    }

    fn instalar_interno(
        raiz_dir: &Path,
        corte_wal: Option<ConfigCorteWal>,
    ) -> (Self, Arc<Mutex<RegistroVfsDir>>) {
        asegurar_obs_vfs_registrado();
        let id = SIGUIENTE_REGLA_OBS_VFS.fetch_add(1, Ordering::Relaxed);
        let subcadena_ruta = raiz_dir
            .file_name()
            .expect("directorio de prueba debe tener nombre")
            .to_string_lossy()
            .into_owned();
        let registro = Arc::new(Mutex::new(RegistroVfsDir::default()));
        REGLAS_OBS_VFS
            .lock()
            .expect("mutex reglas observador VFS")
            .push(ReglaObservadorVfs {
                id,
                subcadena_ruta,
                registro: Arc::clone(&registro),
                corte_wal,
            });
        (Self { id }, registro)
    }
}

impl Drop for GuardiaObservadorVfs {
    fn drop(&mut self) {
        if let Ok(mut reglas) = REGLAS_OBS_VFS.lock() {
            reglas.retain(|r| r.id != self.id);
        }
    }
}

// ===========================================================================
// Regresiones #13 (A..F)
// ===========================================================================

#[test]
fn regresion_a_fk_invalida_solo_en_wal_preserva_tripleta_db_wal_shm() {
    let dir = DirectorioPrueba::nuevo("regresion_a_fk_wal");
    ejecutar_subproceso_cierre_abrupto("fk_invalida_wal", dir.ruta());

    assert!(dir.ruta().exists(), "debe existir sgt.db");
    assert!(dir.ruta_wal().exists(), "debe existir sgt.db-wal");
    assert!(dir.ruta_shm().exists(), "debe existir sgt.db-shm");

    // Confirmar que sgt.db aislada (sin -wal) no contiene la FK inválida ni turnos.
    let copia_aislada = dir.raiz.join("aislada_fk_check.db");
    fs::copy(dir.ruta(), &copia_aislada).expect("debe copiar sgt.db aislada");
    {
        let conn = abrir_solo_lectura_sin_wal(&copia_aislada);
        let turnos: i64 = conn
            .query_row("SELECT COUNT(*) FROM turnos", [], |r| r.get(0))
            .expect("debe contar turnos en DB aislada");
        assert_eq!(
            turnos, 0,
            "la fila con FK inválida debe residir solo en -wal pendiente"
        );
    }
    let _ = fs::remove_file(&copia_aislada);

    let bytes_db_antes = fs::read(dir.ruta()).expect("debe leer sgt.db");
    let bytes_wal_antes = fs::read(dir.ruta_wal()).expect("debe leer sgt.db-wal");
    let bytes_shm_antes = fs::read(dir.ruta_shm()).expect("debe leer sgt.db-shm");

    let err = RepositorioSqlite::abrir(dir.ruta())
        .err()
        .expect("FK inválida en WAL pendiente debe rechazarse en abrir()");
    assert_eq!(err.codigo, CodigoError::BaseDatosCorrupta);

    assert_eq!(
        fs::read(dir.ruta()).expect("sgt.db"),
        bytes_db_antes,
        "sgt.db debe permanecer idéntico byte a byte"
    );
    assert_eq!(
        fs::read(dir.ruta_wal()).expect("sgt.db-wal"),
        bytes_wal_antes,
        "sgt.db-wal debe permanecer idéntico byte a byte"
    );
    assert_eq!(
        fs::read(dir.ruta_shm()).expect("sgt.db-shm"),
        bytes_shm_antes,
        "sgt.db-shm debe permanecer idéntico byte a byte"
    );
}

#[test]
fn regresion_b_wal_valido_con_btree_corrupto_en_db_preserva_tripleta() {
    let dir = DirectorioPrueba::nuevo("regresion_b_btree_con_wal");

    // Ciclo 1 limpio: siembra 5 turnos para que el archivo principal tenga varias páginas integradas.
    {
        let mut repo = RepositorioSqlite::abrir(dir.ruta()).expect("debe abrir BD inicial");
        let mut sistema = repo.cargar().expect("debe cargar inicial");
        let mut turnos = Vec::new();
        for idx in 1..=5_u32 {
            turnos.push(
                sistema
                    .registrar_turno_en(instante_fijo(idx))
                    .expect("debe registrar turno"),
            );
        }
        repo.guardar(&Cambios {
            turnos,
            ventanillas: vec![],
            ultima_secuencia: sistema.ultima_secuencia(),
        })
        .expect("debe persistir T-0001..T-0005");
    }

    // Ciclo 2 abrupto: agrega T-0006 solo en -wal, dejando una tripleta válida pendiente.
    ejecutar_subproceso_cierre_abrupto("btree_corrupto_wal_preparar", dir.ruta());
    assert!(dir.ruta_wal().exists(), "debe existir -wal pendiente");
    assert!(dir.ruta_shm().exists(), "debe existir -shm");

    // Corromper la página 2 (tabla schema_migrations, no reescrita en el WAL pendiente) en sgt.db.
    let mut bytes_db = fs::read(dir.ruta()).expect("debe leer sgt.db");
    assert!(bytes_db.len() >= 8192);
    assert_eq!(bytes_db[18], 2, "cabecera debe permanecer en modo WAL");
    for byte in &mut bytes_db[4096..4160] {
        *byte = 0xFF;
    }
    fs::write(dir.ruta(), &bytes_db).expect("debe escribir B-tree corrupto en sgt.db");

    let bytes_wal_antes = fs::read(dir.ruta_wal()).expect("debe leer sgt.db-wal");
    let bytes_shm_antes = fs::read(dir.ruta_shm()).expect("debe leer sgt.db-shm");

    let err = RepositorioSqlite::abrir(dir.ruta())
        .err()
        .expect("B-tree corrupto en sgt.db con WAL válido pendiente debe rechazarse");
    assert_eq!(err.codigo, CodigoError::BaseDatosCorrupta);

    assert_eq!(
        fs::read(dir.ruta()).expect("sgt.db"),
        bytes_db,
        "sgt.db debe permanecer idéntico byte a byte"
    );
    assert_eq!(
        fs::read(dir.ruta_wal()).expect("sgt.db-wal"),
        bytes_wal_antes,
        "sgt.db-wal debe permanecer idéntico byte a byte"
    );
    assert_eq!(
        fs::read(dir.ruta_shm()).expect("sgt.db-shm"),
        bytes_shm_antes,
        "sgt.db-shm debe permanecer idéntico byte a byte"
    );
}

#[test]
fn regresion_c_cabecera_wal_y_btree_corrupto_sin_sidecars_no_abre_ni_crea_sidecars_vfs() {
    let dir = DirectorioPrueba::nuevo("regresion_c_wal_sin_sidecars_vfs");
    {
        let mut repo = RepositorioSqlite::abrir(dir.ruta()).expect("debe crear BD en WAL");
        let mut sistema = repo.cargar().expect("debe cargar inicial");
        let t1 = sistema
            .registrar_turno_en(instante_fijo(1))
            .expect("T-0001");
        repo.guardar(&Cambios {
            turnos: vec![t1],
            ventanillas: vec![],
            ultima_secuencia: 1,
        })
        .expect("debe guardar T-0001");
    }
    assert_sin_sidecars(&dir);

    let mut bytes = fs::read(dir.ruta()).expect("debe leer sgt.db");
    assert!(bytes.len() >= 8192);
    assert_eq!(bytes[18], 2, "cabecera debe indicar modo WAL");
    for byte in &mut bytes[4096..4160] {
        *byte = 0xFF;
    }
    fs::write(dir.ruta(), &bytes).expect("debe escribir sgt.db con B-tree corrupto");

    let (guardia, registro) = GuardiaObservadorVfs::instalar(&dir);
    let err = RepositorioSqlite::abrir(dir.ruta())
        .err()
        .expect("cabecera WAL + B-tree corrupto debe fallar en abrir()");
    assert_eq!(err.codigo, CodigoError::BaseDatosCorrupta);
    drop(guardia);

    let log = registro.lock().expect("registro VFS");
    assert!(
        log.aperturas.is_empty(),
        "no debe abrir vía VFS ni sgt.db ni sidecars originales durante verificación en sonda: {:?}",
        log.aperturas
    );
    assert!(
        log.accesos.is_empty(),
        "no debe consultar vía VFS los sidecars originales: {:?}",
        log.accesos
    );
    assert!(
        log.borrados.is_empty(),
        "no debe eliminar vía VFS ningún archivo del directorio original: {:?}",
        log.borrados
    );
    assert!(
        log.shm_maps.is_empty(),
        "no debe mapear -shm en el directorio original: {:?}",
        log.shm_maps
    );

    assert_eq!(
        fs::read(dir.ruta()).expect("sgt.db"),
        bytes,
        "sgt.db debe permanecer idéntico byte a byte"
    );
    assert_sin_sidecars(&dir);
}

#[test]
fn regresion_d_migracion_2_en_delete_y_wal_pendiente_rechaza_y_drop_preserva_bytes() {
    // 1. Modo DELETE con schema_migrations = [2].
    {
        let dir = DirectorioPrueba::nuevo("regresion_d_migracion_2_delete");
        crear_base_valida_en_modo_delete(dir.ruta(), 1);
        {
            let conn = Connection::open_with_flags(
                dir.ruta(),
                OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )
            .expect("debe abrir en modo DELETE");
            conn.execute(
                "UPDATE schema_migrations SET version = 2, nombre = '002_ajena' WHERE version = 1",
                [],
            )
            .expect("debe establecer migración 2 en DELETE");
            conn.close().expect("debe cerrar conexión DELETE");
        }
        assert_sin_sidecars(&dir);
        let bytes_antes = fs::read(dir.ruta()).expect("debe leer sgt.db en DELETE");
        assert_eq!(bytes_antes[18], 1, "cabecera debe seguir en DELETE");

        {
            let mut repo = RepositorioSqlite::abrir(dir.ruta())
                .expect("abrir() en migración 2 DELETE debe pasar integridad física");
            let err = repo
                .cargar()
                .expect_err("cargar() debe rechazar migración lógica [2]");
            assert_eq!(err.codigo, CodigoError::BaseDatosCorrupta);
        }

        assert_sin_sidecars(&dir);
        assert_eq!(
            fs::read(dir.ruta()).expect("sgt.db tras drop"),
            bytes_antes,
            "drop(repo) tras fallo en cargar() debe preservar bytes exactos de DB en DELETE"
        );
    }

    // 2. Modo WAL con frames pendientes y schema_migrations = [2] en -wal.
    {
        let dir = DirectorioPrueba::nuevo("regresion_d_migracion_2_wal");
        ejecutar_subproceso_cierre_abrupto("migracion_2_wal", dir.ruta());
        assert!(dir.ruta_wal().exists(), "debe existir -wal pendiente");

        // Verificar que sgt.db aún tenía versión 1 (el cambio a [2] reside en -wal pendiente).
        let copia_aislada = dir.raiz.join("aislada_migracion_1.db");
        fs::copy(dir.ruta(), &copia_aislada).expect("debe copiar sgt.db aislada");
        {
            let conn = abrir_solo_lectura_sin_wal(&copia_aislada);
            let version_en_db: i64 = conn
                .query_row("SELECT version FROM schema_migrations", [], |r| r.get(0))
                .expect("debe leer versión en sgt.db aislada");
            assert_eq!(version_en_db, 1);
        }
        let _ = fs::remove_file(&copia_aislada);

        let bytes_db_antes = fs::read(dir.ruta()).expect("debe leer sgt.db");
        let bytes_wal_antes = fs::read(dir.ruta_wal()).expect("debe leer sgt.db-wal");

        {
            let mut repo = RepositorioSqlite::abrir(dir.ruta())
                .expect("abrir() en migración 2 con WAL pendiente debe pasar integridad física");
            let err = repo
                .cargar()
                .expect_err("cargar() debe rechazar migración lógica [2] desde WAL pendiente");
            assert_eq!(err.codigo, CodigoError::BaseDatosCorrupta);
        }

        assert_eq!(
            fs::read(dir.ruta()).expect("sgt.db tras drop"),
            bytes_db_antes,
            "drop(repo) debe preservar bytes exactos de DB con WAL pendiente (NO_CKPT_ON_CLOSE)"
        );
        assert_eq!(
            fs::read(dir.ruta_wal()).expect("sgt.db-wal tras drop"),
            bytes_wal_antes,
            "drop(repo) debe preservar bytes exactos de WAL pendiente"
        );
    }
}

#[test]
fn regresion_e_wal_corrupto_preserva_tripleta_y_wal_reciclado_restart_recupera() {
    // 1. Generar origen con T-0001 confirmado únicamente en -wal.
    let origen = DirectorioPrueba::nuevo("regresion_e_origen_t0001");
    ejecutar_subproceso_cierre_abrupto("solo_t0001_wal", origen.ruta());
    assert!(origen.ruta().exists());
    assert!(origen.ruta_wal().exists());
    assert!(origen.ruta_shm().exists());

    let copia_aislada = origen.raiz.join("aislada_sin_wal.db");
    fs::copy(origen.ruta(), &copia_aislada).expect("debe copiar sgt.db aislada");
    {
        let conn = abrir_solo_lectura_sin_wal(&copia_aislada);
        let turnos: i64 = conn
            .query_row("SELECT COUNT(*) FROM turnos", [], |r| r.get(0))
            .expect("debe contar turnos en DB aislada");
        assert_eq!(turnos, 0, "T-0001 debe existir únicamente en -wal");
    }
    let _ = fs::remove_file(&copia_aislada);

    // 2. Copia control sana: debe recuperar T-0001.
    {
        let ctrl = DirectorioPrueba::nuevo("regresion_e_control_sana");
        origen.copiar_tripleta_a(&ctrl);

        let mut repo = RepositorioSqlite::abrir(ctrl.ruta())
            .expect("copia control sana debe abrir y recuperar WAL");
        let sistema = repo.cargar().expect("copia control sana debe cargar");
        assert_eq!(sistema.ultima_secuencia(), 1);
        let t1 = sistema.buscar_turno("T-0001").expect("T-0001 recuperado");
        assert_eq!(t1.estado, EstadoTurno::EnEspera);
    }

    // 3. Copias con WAL corrupto: magia inválida, payload corrupto, checksum de frame corrupto,
    //    y frame truncado; todas deben devolver BaseDatosCorrupta preservando DB/WAL/SHM.
    let mutar_y_verificar_corrupta = |etiqueta: &str, mutar_wal: &dyn Fn(&mut Vec<u8>)| {
        let dir = DirectorioPrueba::nuevo(etiqueta);
        origen.copiar_tripleta_a(&dir);

        let mut bytes_wal = fs::read(dir.ruta_wal()).expect("debe leer sgt.db-wal");
        mutar_wal(&mut bytes_wal);
        fs::write(dir.ruta_wal(), &bytes_wal).expect("debe escribir sgt.db-wal mutado");

        let bytes_db_antes = fs::read(dir.ruta()).expect("debe leer sgt.db");
        let bytes_shm_antes = fs::read(dir.ruta_shm()).expect("debe leer sgt.db-shm");

        let err = RepositorioSqlite::abrir(dir.ruta())
            .err()
            .expect("WAL corrupto debe rechazarse en abrir()");
        assert_eq!(err.codigo, CodigoError::BaseDatosCorrupta);

        assert_eq!(fs::read(dir.ruta()).expect("sgt.db"), bytes_db_antes);
        assert_eq!(fs::read(dir.ruta_wal()).expect("sgt.db-wal"), bytes_wal);
        assert_eq!(
            fs::read(dir.ruta_shm()).expect("sgt.db-shm"),
            bytes_shm_antes
        );
    };

    // 3a. Magia WAL inválida.
    mutar_y_verificar_corrupta("regresion_e_magia_invalida", &|wal| {
        wal[0..4].copy_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF]);
    });

    // 3b. Payload de frame corrupto (checksum almacenado deja de coincidir).
    mutar_y_verificar_corrupta("regresion_e_payload_corrupto", &|wal| {
        assert!(wal.len() >= 32 + 24 + 64);
        wal[32 + 24 + 42] ^= 0x5A;
    });

    // 3c. Checksum de frame corrupto.
    mutar_y_verificar_corrupta("regresion_e_checksum_frame_corrupto", &|wal| {
        assert!(wal.len() >= 32 + 24);
        wal[32 + 17] ^= 0xA5;
    });

    // 3d. Cabecera WAL truncada (< 32 bytes) y checksum de cabecera corrupto:
    //     siguen rechazándose con BaseDatosCorrupta preservando la tripleta.
    mutar_y_verificar_corrupta("regresion_e_cabecera_truncada", &|wal| {
        assert!(wal.len() >= 32);
        wal.truncate(16);
    });
    mutar_y_verificar_corrupta("regresion_e_checksum_cabecera_corrupto", &|wal| {
        assert!(wal.len() >= 32);
        wal[25] ^= 0x55;
    });

    // 3e. Truncado del frame final de commit (-10 bytes): en disco es indistinguible
    //     de un crash ocurrido antes de completar el COMMIT de T-0001; bajo semántica
    //     SQLite WAL coherente, abrir() y cargar() revierten la transacción sin
    //     cierre al estado previo (0 turnos, secuencia 0).
    {
        let dir = DirectorioPrueba::nuevo("regresion_e_commit_truncado");
        origen.copiar_tripleta_a(&dir);

        let mut bytes_wal = fs::read(dir.ruta_wal()).expect("debe leer sgt.db-wal");
        assert!(bytes_wal.len() > 64);
        bytes_wal.truncate(bytes_wal.len() - 10);
        fs::write(dir.ruta_wal(), &bytes_wal).expect("debe escribir sgt.db-wal truncado");

        let mut repo = RepositorioSqlite::abrir(dir.ruta()).expect(
            "frame de commit truncado (indistinguible de crash previo al commit) debe abrir",
        );
        let sistema = repo
            .cargar()
            .expect("debe restaurar el estado previo al commit no completado");
        assert_eq!(sistema.ultima_secuencia(), 0);
        assert!(
            sistema.consultar_historial().is_empty(),
            "T-0001 cuyo commit quedó truncado no debe darse por confirmada"
        );
    }

    // 4. WAL reciclado tras PRAGMA wal_checkpoint(RESTART) con T-0001..15 integrados y T-0016
    //    como commit menor, dejando frames residuales del salt anterior en la cola del -wal:
    //    - Copia sana: recupera los 16 turnos (ultima_secuencia = 16).
    //    - Copia dañada A: último frame activo con 1 byte de salt alterado -> BaseDatosCorrupta,
    //      preservando DB/WAL/SHM byte a byte.
    //    - Copia dañada B: último frame activo con los 8 bytes de salt reemplazados por el salt
    //      antiguo de la cola -> BaseDatosCorrupta, preservando DB/WAL/SHM byte a byte.
    {
        let origen_reciclado = DirectorioPrueba::nuevo("regresion_e_wal_reciclado_origen");
        ejecutar_subproceso_cierre_abrupto("wal_reciclado_restart", origen_reciclado.ruta());

        let aislada = origen_reciclado.raiz.join("aislada_reciclado.db");
        fs::copy(origen_reciclado.ruta(), &aislada).expect("debe copiar sgt.db aislada");
        {
            let conn = abrir_solo_lectura_sin_wal(&aislada);
            assert_eq!(leer_ultima_secuencia_sql(&conn), 15);
            let count: i64 = conn
                .query_row("SELECT COUNT(*) FROM turnos", [], |r| r.get(0))
                .expect("debe contar turnos integrados previos al commit menor");
            assert_eq!(
                count, 15,
                "T-0016 debe residir aún solo en el WAL reciclado"
            );
        }
        let _ = fs::remove_file(&aislada);

        // 4a. Copia sana: recupera los 16 turnos.
        {
            let ctrl = DirectorioPrueba::nuevo("regresion_e_wal_reciclado_sana");
            origen_reciclado.copiar_tripleta_a(&ctrl);

            let mut repo = RepositorioSqlite::abrir(ctrl.ruta())
                .expect("WAL reciclado tras RESTART con cola del salt anterior debe abrirse");
            let sistema = repo
                .cargar()
                .expect("WAL reciclado debe recuperar todos los turnos");
            assert_eq!(sistema.ultima_secuencia(), 16);
            assert_eq!(sistema.consultar_historial().len(), 16);
            assert!(sistema.buscar_turno("T-0016").is_some());
        }

        // Localizar último frame activo y salt antiguo en el WAL reciclado.
        let wal_origen = fs::read(origen_reciclado.ruta_wal()).expect("debe leer -wal reciclado");
        let db_origen = fs::read(origen_reciclado.ruta()).expect("debe leer sgt.db");
        let tam_pagina = usize::from(u16::from_be_bytes([db_origen[16], db_origen[17]]));
        let frame_len = 24 + tam_pagina;
        let num_frames = (wal_origen.len() - 32) / frame_len;
        let salt_actual: [u8; 8] = wal_origen[16..24].try_into().expect("salt actual");
        let mut idx_ultimo_activo = None;
        for idx in 0..num_frames {
            let inicio = 32 + idx * frame_len;
            if wal_origen[inicio + 8..inicio + 16] == salt_actual {
                idx_ultimo_activo = Some(idx);
            } else {
                break;
            }
        }
        let idx_ultimo_activo =
            idx_ultimo_activo.expect("debe haber al menos un frame activo con salt_actual");
        assert!(
            idx_ultimo_activo + 1 < num_frames,
            "debe quedar al menos un frame antiguo después del último activo"
        );
        let inicio_ultimo_activo = 32 + idx_ultimo_activo * frame_len;
        let inicio_ultimo_frame = 32 + (num_frames - 1) * frame_len;
        let salt_antiguo: [u8; 8] = wal_origen[inicio_ultimo_frame + 8..inicio_ultimo_frame + 16]
            .try_into()
            .expect("salt antiguo");
        assert_ne!(salt_antiguo, salt_actual);

        // 4b. Último frame activo con 1 byte de salt alterado.
        {
            let dir = DirectorioPrueba::nuevo("regresion_e_wal_reciclado_salt_1byte");
            origen_reciclado.copiar_tripleta_a(&dir);

            let mut wal_mutado = wal_origen.clone();
            wal_mutado[inicio_ultimo_activo + 8] ^= 0x01;
            fs::write(dir.ruta_wal(), &wal_mutado)
                .expect("debe escribir -wal mutado (1 byte salt)");

            let bytes_db_antes = fs::read(dir.ruta()).expect("debe leer sgt.db");
            let bytes_shm_antes = fs::read(dir.ruta_shm()).expect("debe leer sgt.db-shm");

            let err = RepositorioSqlite::abrir(dir.ruta())
                .err()
                .expect("1 byte de salt alterado en último frame activo debe rechazarse");
            assert_eq!(err.codigo, CodigoError::BaseDatosCorrupta);
            assert_eq!(fs::read(dir.ruta()).expect("sgt.db"), bytes_db_antes);
            assert_eq!(fs::read(dir.ruta_wal()).expect("sgt.db-wal"), wal_mutado);
            assert_eq!(
                fs::read(dir.ruta_shm()).expect("sgt.db-shm"),
                bytes_shm_antes
            );
        }

        // 4c. Último frame activo con 8 bytes de salt reemplazados por salt antiguo.
        {
            let dir = DirectorioPrueba::nuevo("regresion_e_wal_reciclado_salt_antiguo");
            origen_reciclado.copiar_tripleta_a(&dir);

            let mut wal_mutado = wal_origen.clone();
            wal_mutado[inicio_ultimo_activo + 8..inicio_ultimo_activo + 16]
                .copy_from_slice(&salt_antiguo);
            fs::write(dir.ruta_wal(), &wal_mutado)
                .expect("debe escribir -wal mutado (8 bytes salt antiguo)");

            let bytes_db_antes = fs::read(dir.ruta()).expect("debe leer sgt.db");
            let bytes_shm_antes = fs::read(dir.ruta_shm()).expect("debe leer sgt.db-shm");

            let err = RepositorioSqlite::abrir(dir.ruta())
                .err()
                .expect("8 bytes de salt antiguo en último frame activo debe rechazarse");
            assert_eq!(err.codigo, CodigoError::BaseDatosCorrupta);
            assert_eq!(fs::read(dir.ruta()).expect("sgt.db"), bytes_db_antes);
            assert_eq!(fs::read(dir.ruta_wal()).expect("sgt.db-wal"), wal_mutado);
            assert_eq!(
                fs::read(dir.ruta_shm()).expect("sgt.db-shm"),
                bytes_shm_antes
            );
        }
    }
}

#[test]
fn regresion_f_fk_mismatch_turnos_sin_pk_rechaza_y_preserva_archivo() {
    let dir = DirectorioPrueba::nuevo("regresion_f_fk_mismatch");
    crear_base_valida_en_modo_delete(dir.ruta(), 0);

    {
        let conn = Connection::open_with_flags(
            dir.ruta(),
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .expect("debe abrir BD en modo DELETE");
        conn.execute_batch(
            "PRAGMA foreign_keys = OFF;
             DROP TABLE turnos;
             CREATE TABLE turnos (
                 id                  TEXT    NOT NULL,
                 secuencia           INTEGER NOT NULL UNIQUE,
                 fecha_hora_registro TEXT    NOT NULL,
                 estado              TEXT    NOT NULL,
                 ventanilla_asignada TEXT    NULL,
                 FOREIGN KEY (ventanilla_asignada) REFERENCES ventanillas(id) ON DELETE RESTRICT
             );",
        )
        .expect("debe recrear tabla turnos sin PK con foreign_keys = OFF");
        conn.close()
            .expect("debe cerrar conexión tras recrear turnos sin PK");
    }

    assert_sin_sidecars(&dir);
    let bytes_antes = fs::read(dir.ruta()).expect("debe leer bytes antes de abrir");

    let err = RepositorioSqlite::abrir(dir.ruta())
        .err()
        .expect("FK mismatch estructural debe rechazarse en abrir()");
    assert_eq!(err.codigo, CodigoError::BaseDatosCorrupta);
    assert_eq!(
        fs::read(dir.ruta()).expect("debe conservar archivo idéntico tras rechazo FK mismatch"),
        bytes_antes
    );
    assert_sin_sidecars(&dir);
}

#[test]
fn regresion_g_vfs_cierre_abrupto_wal_cabecera_32_y_corte_parcial_12_recupera() {
    // 1. VFS cierre abrupto dejando WAL con 32 bytes de cabecera válida y 0 frames:
    //    debe abrir y recuperar estado limpio (secuencia 0, 0 turnos) sin error.
    {
        let dir = DirectorioPrueba::nuevo("regresion_g_vfs_wal_32");
        ejecutar_subproceso_cierre_abrupto("vfs_wal_cabecera_32_sin_frames", dir.ruta());

        let tam_wal = fs::metadata(dir.ruta_wal())
            .expect("debe existir -wal tras corte VFS")
            .len();
        assert_eq!(tam_wal, 32, "el -wal debe tener exactamente 32 bytes");

        let mut repo = RepositorioSqlite::abrir(dir.ruta())
            .expect("WAL de 32 bytes (cabecera válida sin frames) debe abrir sin error");
        let sistema = repo
            .cargar()
            .expect("debe cargar estado limpio desde WAL de 32 bytes");
        assert_eq!(sistema.ultima_secuencia(), 0);
        assert!(sistema.consultar_historial().is_empty());
    }

    // 2. VFS cierre abrupto tras confirmar T-0001 y cortar a 12 bytes el frame siguiente
    //    de T-0002: debe abrir y recuperar T-0001 sin error, descartando la escritura parcial.
    {
        let dir = DirectorioPrueba::nuevo("regresion_g_vfs_wal_corte_12");
        ejecutar_subproceso_cierre_abrupto("vfs_wal_corte_parcial_12_tras_t0001", dir.ruta());

        let bytes_db = fs::read(dir.ruta()).expect("debe leer sgt.db");
        let tam_pagina = u64::from(u16::from_be_bytes([bytes_db[16], bytes_db[17]]));
        let frame_len = 24 + tam_pagina;
        let tam_wal = fs::metadata(dir.ruta_wal())
            .expect("debe existir -wal tras corte parcial VFS")
            .len();
        assert!(tam_wal > 32 + frame_len);
        assert_eq!(
            (tam_wal - 32) % frame_len,
            12,
            "el -wal debe terminar con un corte parcial exacto de 12 bytes"
        );

        {
            let mut repo = RepositorioSqlite::abrir(dir.ruta()).expect(
                "WAL con T-0001 confirmada y corte parcial de 12 bytes debe abrir sin error",
            );
            let sistema = repo
                .cargar()
                .expect("debe recuperar T-0001 confirmada ignorando el corte parcial de 12 bytes");
            assert_eq!(sistema.ultima_secuencia(), 1);
            let t1 = sistema
                .buscar_turno("T-0001")
                .expect("T-0001 confirmada debe recuperarse");
            assert_eq!(t1.estado, EstadoTurno::EnEspera);
            assert!(
                sistema.buscar_turno("T-0002").is_none(),
                "T-0002 cortada a mitad de frame no debe existir"
            );
        }

        let mut reabierto =
            RepositorioSqlite::abrir(dir.ruta()).expect("debe reabrir tras recuperación");
        let sistema_reabierto = reabierto.cargar().expect("debe conservar T-0001");
        assert_eq!(sistema_reabierto.ultima_secuencia(), 1);
        assert_eq!(sistema_reabierto.consultar_historial().len(), 1);
    }
}

#[test]
fn regresion_h_migracion_2_wal_sin_sidecars_vfs_cero_sidecars_y_bytes_identicos() {
    let dir = DirectorioPrueba::nuevo("regresion_h_migracion_2_wal_sin_sidecars");
    {
        let mut repo = RepositorioSqlite::abrir(dir.ruta()).expect("debe crear BD WAL inicial");
        let mut sistema = repo.cargar().expect("debe cargar inicial");
        let t1 = sistema
            .registrar_turno_en(instante_fijo(1))
            .expect("T-0001");
        repo.guardar(&Cambios {
            turnos: vec![t1],
            ventanillas: vec![],
            ultima_secuencia: 1,
        })
        .expect("debe guardar T-0001");
    }
    {
        let conn = abrir_conexion_auxiliar(dir.ruta());
        conn.execute(
            "UPDATE schema_migrations SET version = 2, nombre = '002_ajena' WHERE version = 1",
            [],
        )
        .expect("debe establecer schema_migrations = [2]");
        conn.close().expect("debe cerrar conexión WAL limpiamente");
    }
    assert_sin_sidecars(&dir);

    let bytes_antes = fs::read(dir.ruta()).expect("debe leer sgt.db");
    assert_eq!(bytes_antes[18], 2, "cabecera debe indicar lectura WAL");
    assert_eq!(bytes_antes[19], 2, "cabecera debe indicar escritura WAL");

    let (guardia, registro) = GuardiaObservadorVfs::instalar(&dir);
    {
        let mut repo = RepositorioSqlite::abrir(dir.ruta()).expect(
            "abrir() en cabecera WAL sin sidecars con migración [2] debe pasar integridad física",
        );
        let err = repo
            .cargar()
            .expect_err("cargar() debe rechazar schema_migrations = [2] con BaseDatosCorrupta");
        assert_eq!(err.codigo, CodigoError::BaseDatosCorrupta);
    }
    drop(guardia);

    let log = registro.lock().expect("registro VFS");
    assert!(
        log.aperturas.is_empty(),
        "no debe abrir ni crear transitoriamente archivos ni sidecars en directorio original: {:?}",
        log.aperturas
    );
    assert!(
        log.accesos.is_empty(),
        "no debe consultar sidecars vía VFS en directorio original: {:?}",
        log.accesos
    );
    assert!(
        log.shm_maps.is_empty(),
        "no debe mapear/crear -shm transitoriamente en directorio original: {:?}",
        log.shm_maps
    );
    assert!(
        log.borrados.is_empty(),
        "no debe borrar archivos del directorio original: {:?}",
        log.borrados
    );

    assert_sin_sidecars(&dir);
    assert_eq!(
        fs::read(dir.ruta()).expect("sgt.db tras drop(repo)"),
        bytes_antes,
        "drop(repo) debe preservar bytes exactos de sgt.db"
    );
}

fn checksum_wal_prueba(datos: &[u8], little: bool, suma: &mut (u32, u32)) {
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

struct ResumenWalPrueba {
    num_frames: usize,
    frame_len: usize,
    salt_actual: [u8; 8],
    commits_activos: Vec<usize>,
    primer_frame_invalido: Option<usize>,
    commits_en_cola_residual: usize,
}

fn inspeccionar_wal_prueba(wal: &[u8], tam_pagina: usize) -> ResumenWalPrueba {
    assert!(wal.len() >= 32, "WAL debe incluir cabecera de 32 bytes");
    let little = match u32::from_be_bytes(wal[..4].try_into().expect("magia")) {
        0x377f_0682 => true,
        0x377f_0683 => false,
        otro => panic!("magia WAL inesperada: {otro:#x}"),
    };
    let mut suma = (0_u32, 0_u32);
    checksum_wal_prueba(&wal[..24], little, &mut suma);
    assert_eq!(
        suma,
        (
            u32::from_be_bytes(wal[24..28].try_into().expect("cksum0")),
            u32::from_be_bytes(wal[28..32].try_into().expect("cksum1")),
        ),
        "checksum de cabecera WAL debe ser válido"
    );

    let frame_len = 24 + tam_pagina;
    let num_frames = (wal.len() - 32) / frame_len;
    let salt_actual: [u8; 8] = wal[16..24].try_into().expect("salt cabecera");
    let mut commits_activos = Vec::new();
    let mut primer_frame_invalido = None;
    let mut commits_en_cola_residual = 0;
    let mut en_cadena_activa = true;

    for idx in 0..num_frames {
        let inicio = 32 + idx * frame_len;
        let frame = &wal[inicio..inicio + frame_len];
        let tam_commit = u32::from_be_bytes(frame[4..8].try_into().expect("tam_commit"));
        let almacenada = (
            u32::from_be_bytes(frame[16..20].try_into().expect("f_cksum0")),
            u32::from_be_bytes(frame[20..24].try_into().expect("f_cksum1")),
        );
        if en_cadena_activa && frame[8..16] == salt_actual {
            let mut siguiente = suma;
            checksum_wal_prueba(&frame[..8], little, &mut siguiente);
            checksum_wal_prueba(&frame[24..], little, &mut siguiente);
            if siguiente == almacenada {
                suma = siguiente;
                if tam_commit != 0 {
                    commits_activos.push(idx);
                }
                continue;
            }
            en_cadena_activa = false;
            primer_frame_invalido = Some(idx);
        } else if en_cadena_activa {
            en_cadena_activa = false;
            primer_frame_invalido = Some(idx);
        }
        if tam_commit != 0 {
            commits_en_cola_residual += 1;
        }
    }

    ResumenWalPrueba {
        num_frames,
        frame_len,
        salt_actual,
        commits_activos,
        primer_frame_invalido,
        commits_en_cola_residual,
    }
}

#[test]
fn regresion_i_rollback_tras_spill_wal_recupera_y_detecta_corrupcion_en_frames_t2() {
    // 1. Variante: cierre abrupto justo tras el rollback con spill (sin guardar T-0002).
    //    Debe existir WAL residual con frames no confirmados y recuperar secuencia 1 (solo T-0001).
    {
        let dir = DirectorioPrueba::nuevo("regresion_i_spill_sin_t2");
        ejecutar_subproceso_cierre_abrupto("regresion_i_spill_sin_t0002", dir.ruta());

        let bytes_db = fs::read(dir.ruta()).expect("debe leer sgt.db");
        let bytes_wal = fs::read(dir.ruta_wal()).expect("debe leer sgt.db-wal");
        let tam_pagina = usize::from(u16::from_be_bytes([bytes_db[16], bytes_db[17]]));
        let resumen = inspeccionar_wal_prueba(&bytes_wal, tam_pagina);
        assert_eq!(
            resumen.commits_activos.len(),
            1,
            "solo debe haber 1 frame de commit activo (T-0001) antes de los frames residuales del spill"
        );
        let idx_commit_t1 = resumen.commits_activos[0];
        assert!(
            resumen.num_frames > idx_commit_t1 + 50,
            "el WAL debe conservar frames residuales no confirmados del spill tras T-0001 (frames={}, commit_t1={idx_commit_t1})",
            resumen.num_frames
        );
        assert_eq!(
            resumen.commits_en_cola_residual, 0,
            "ningún frame residual tras T-0001 debe marcar commit"
        );
        for idx in (idx_commit_t1 + 1)..resumen.num_frames {
            let inicio = 32 + idx * resumen.frame_len;
            let tam_commit = u32::from_be_bytes(
                bytes_wal[inicio + 4..inicio + 8]
                    .try_into()
                    .expect("tam_commit"),
            );
            assert_eq!(
                tam_commit, 0,
                "frame residual {idx} del spill revertido no debe tener tam_commit != 0"
            );
        }

        let aislada = dir.raiz.join("aislada_sin_t2.db");
        fs::copy(dir.ruta(), &aislada).expect("debe copiar sgt.db aislada");
        {
            let conn = abrir_solo_lectura_sin_wal(&aislada);
            let count: i64 = conn
                .query_row("SELECT COUNT(*) FROM turnos", [], |r| r.get(0))
                .expect("debe contar turnos en sgt.db aislada");
            assert_eq!(count, 0, "T-0001 debe residir aún en -wal");
        }
        let _ = fs::remove_file(&aislada);

        {
            let mut repo = RepositorioSqlite::abrir(dir.ruta())
                .expect("abrir() tras rollback con spill y cierre abrupto debe ser Ok");
            let sistema = repo
                .cargar()
                .expect("cargar() tras rollback con spill debe recuperar secuencia 1");
            assert_eq!(sistema.ultima_secuencia(), 1);
            assert_eq!(sistema.consultar_historial().len(), 1);
            assert!(sistema.buscar_turno("T-0001").is_some());
            assert!(sistema.buscar_turno("T-0002").is_none());
        }

        let mut reabierto =
            RepositorioSqlite::abrir(dir.ruta()).expect("debe reabrir tras recuperación");
        let sistema_reabierto = reabierto.cargar().expect("debe conservar secuencia 1");
        assert_eq!(sistema_reabierto.ultima_secuencia(), 1);
        assert_eq!(sistema_reabierto.consultar_historial().len(), 1);
    }

    // 2. Flujo principal: T-0001 confirmada -> spill (~50000 turnos + id vacío) y rollback
    //    -> cargar() confirma seq=1 -> T-0002 guardada en la misma instancia -> cierre abrupto.
    let origen = DirectorioPrueba::nuevo("regresion_i_spill_con_t2_origen");
    ejecutar_subproceso_cierre_abrupto("regresion_i_spill_y_t0002", origen.ruta());
    assert!(origen.ruta().exists());
    assert!(origen.ruta_wal().exists());
    assert!(origen.ruta_shm().exists());

    let aislada = origen.raiz.join("aislada_con_t2.db");
    fs::copy(origen.ruta(), &aislada).expect("debe copiar sgt.db aislada");
    {
        let conn = abrir_solo_lectura_sin_wal(&aislada);
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM turnos", [], |r| r.get(0))
            .expect("debe contar turnos en sgt.db aislada");
        assert_eq!(
            count, 0,
            "T-0001 y T-0002 deben residir en -wal antes de recuperar"
        );
    }
    let _ = fs::remove_file(&aislada);

    let db_origen = fs::read(origen.ruta()).expect("debe leer sgt.db origen");
    let wal_origen = fs::read(origen.ruta_wal()).expect("debe leer sgt.db-wal origen");
    let tam_pagina = usize::from(u16::from_be_bytes([db_origen[16], db_origen[17]]));
    let resumen = inspeccionar_wal_prueba(&wal_origen, tam_pagina);
    assert_eq!(
        resumen.commits_activos.len(),
        2,
        "la cadena activa del WAL debe contener exactamente 2 commits (T-0001 y T-0002)"
    );
    let idx_commit_t1 = resumen.commits_activos[0];
    let idx_commit_t2 = resumen.commits_activos[1];
    assert!(
        idx_commit_t2 > idx_commit_t1 + 1,
        "T-0002 debe incluir al menos un frame intermedio antes de su frame de commit (t1={idx_commit_t1}, t2={idx_commit_t2})"
    );
    assert!(
        resumen.num_frames > idx_commit_t2 + 50,
        "el WAL post-T-0002 debe conservar frames residuales no confirmados del spill (frames={}, commit_t2={idx_commit_t2})",
        resumen.num_frames
    );
    assert_eq!(
        resumen.primer_frame_invalido,
        Some(idx_commit_t2 + 1),
        "el frame inmediatamente posterior al commit de T-0002 debe ser el inicio de la cola residual del spill cuya suma ya no encadena con T-0002"
    );
    assert_eq!(
        resumen.commits_en_cola_residual, 0,
        "la cola residual del spill tras T-0002 no debe contener ningún commit"
    );

    // 2a. Copia sana: recupera seq=2 y ambos turnos (T-0001 y T-0002).
    {
        let ctrl = DirectorioPrueba::nuevo("regresion_i_spill_con_t2_sana");
        origen.copiar_tripleta_a(&ctrl);

        {
            let mut repo = RepositorioSqlite::abrir(ctrl.ruta())
                .expect("copia sana con WAL residual tras spill + T-0002 debe abrir");
            let sistema = repo
                .cargar()
                .expect("copia sana debe recuperar seq=2 y ambos turnos");
            assert_eq!(sistema.ultima_secuencia(), 2);
            assert_eq!(sistema.consultar_historial().len(), 2);
            assert_eq!(
                sistema.buscar_turno("T-0001").expect("T-0001").estado,
                EstadoTurno::EnEspera
            );
            assert_eq!(
                sistema.buscar_turno("T-0002").expect("T-0002").estado,
                EstadoTurno::EnEspera
            );
        }

        let mut reabierto =
            RepositorioSqlite::abrir(ctrl.ruta()).expect("debe reabrir copia sana tras cierre");
        let sistema_reabierto = reabierto.cargar().expect("debe conservar T-0001 y T-0002");
        assert_eq!(sistema_reabierto.ultima_secuencia(), 2);
        assert_eq!(sistema_reabierto.consultar_historial().len(), 2);
    }

    // 2b. Copia con payload alterado en un frame intermedio de T-0002: BaseDatosCorrupta
    //     y tripleta DB/WAL/SHM idéntica byte a byte.
    {
        let dir = DirectorioPrueba::nuevo("regresion_i_t2_intermedio_corrupto");
        origen.copiar_tripleta_a(&dir);

        let idx_intermedio = idx_commit_t1 + 1;
        let inicio = 32 + idx_intermedio * resumen.frame_len;
        let mut wal_mutado = wal_origen.clone();
        assert_eq!(
            &wal_mutado[inicio + 8..inicio + 16],
            &resumen.salt_actual,
            "el frame intermedio de T-0002 debe pertenecer al salt vigente"
        );
        assert_eq!(
            u32::from_be_bytes(
                wal_mutado[inicio + 4..inicio + 8]
                    .try_into()
                    .expect("tam_commit")
            ),
            0,
            "el frame intermedio de T-0002 no debe ser frame de commit"
        );
        wal_mutado[inicio + 24 + 42] ^= 0x5A;
        fs::write(dir.ruta_wal(), &wal_mutado)
            .expect("debe escribir -wal con payload intermedio de T-0002 alterado");

        let bytes_db_antes = fs::read(dir.ruta()).expect("debe leer sgt.db");
        let bytes_shm_antes = fs::read(dir.ruta_shm()).expect("debe leer sgt.db-shm");

        let err = RepositorioSqlite::abrir(dir.ruta())
            .err()
            .expect("payload alterado en frame intermedio de T-0002 debe devolverse como corrupta");
        assert_eq!(err.codigo, CodigoError::BaseDatosCorrupta);
        assert_eq!(fs::read(dir.ruta()).expect("sgt.db"), bytes_db_antes);
        assert_eq!(fs::read(dir.ruta_wal()).expect("sgt.db-wal"), wal_mutado);
        assert_eq!(
            fs::read(dir.ruta_shm()).expect("sgt.db-shm"),
            bytes_shm_antes
        );
    }

    // 2c. Copia con payload alterado en el frame de commit de T-0002: BaseDatosCorrupta
    //     y tripleta DB/WAL/SHM idéntica byte a byte.
    {
        let dir = DirectorioPrueba::nuevo("regresion_i_t2_commit_corrupto");
        origen.copiar_tripleta_a(&dir);

        let inicio = 32 + idx_commit_t2 * resumen.frame_len;
        let mut wal_mutado = wal_origen.clone();
        assert_eq!(
            &wal_mutado[inicio + 8..inicio + 16],
            &resumen.salt_actual,
            "el frame de commit de T-0002 debe pertenecer al salt vigente"
        );
        assert_ne!(
            u32::from_be_bytes(
                wal_mutado[inicio + 4..inicio + 8]
                    .try_into()
                    .expect("tam_commit")
            ),
            0,
            "el frame de commit de T-0002 debe tener tam_commit != 0"
        );
        wal_mutado[inicio + 24 + 42] ^= 0x5A;
        fs::write(dir.ruta_wal(), &wal_mutado)
            .expect("debe escribir -wal con payload de commit de T-0002 alterado");

        let bytes_db_antes = fs::read(dir.ruta()).expect("debe leer sgt.db");
        let bytes_shm_antes = fs::read(dir.ruta_shm()).expect("debe leer sgt.db-shm");

        let err = RepositorioSqlite::abrir(dir.ruta())
            .err()
            .expect("payload alterado en frame de commit de T-0002 debe devolverse como corrupta");
        assert_eq!(err.codigo, CodigoError::BaseDatosCorrupta);
        assert_eq!(fs::read(dir.ruta()).expect("sgt.db"), bytes_db_antes);
        assert_eq!(fs::read(dir.ruta_wal()).expect("sgt.db-wal"), wal_mutado);
        assert_eq!(
            fs::read(dir.ruta_shm()).expect("sgt.db-shm"),
            bytes_shm_antes
        );
    }
}
