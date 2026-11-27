//! Pruebas de integración en disco para [`RepositorioSqlite`] (`#12`).
//!
//! Verifican apertura sin reseed silencioso, persistencia real en disco,
//! preservación de secuencia tras reinicio, inmutabilidad del historial
//! terminal, rollback ante fallos SQL, reutilización segura de la conexión,
//! bloqueo por escritura concurrente y mapeo de errores y conversiones.

use std::ffi::{CStr, c_char, c_int, c_void};
use std::fs;
use std::mem::{align_of, size_of};
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{TimeZone, Utc};
use gestor_de_turnos_lib::db;
use gestor_de_turnos_lib::domain::{MAX_SECUENCIA_JS, SistemaTurnos};
use gestor_de_turnos_lib::models::{CodigoError, EstadoTurno, Turno, Ventanilla, VentanillaId};
use gestor_de_turnos_lib::repository::{Cambios, Repositorio, RepositorioSqlite};
use rusqlite::{Connection, ffi};

static CONTADOR_RUTAS: AtomicU64 = AtomicU64::new(0);

/// Directorio temporal aislado por prueba; limpia DB, WAL y SHM al destruirse.
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
            "sgt_repo_test_{etiqueta}_{}_{nanos}_{seq}",
            std::process::id()
        ));
        let ruta_db = raiz.join("sgt.db");
        Self { raiz, ruta_db }
    }

    fn ruta(&self) -> &Path {
        &self.ruta_db
    }
}

impl Drop for DirectorioPrueba {
    fn drop(&mut self) {
        for sufijo in ["", "-wal", "-shm"] {
            let archivo = PathBuf::from(format!("{}{sufijo}", self.ruta_db.display()));
            let _ = fs::remove_file(archivo);
        }
        let _ = fs::remove_dir_all(&self.raiz);
    }
}

fn instante_fijo(segundo: u32) -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 11, 26, 12, 0, segundo)
        .single()
        .expect("fecha UTC válida")
}

fn abrir_segunda_conexion(ruta: &Path) -> Connection {
    db::abrir_conexion_configurada(ruta).expect("debe abrir conexión auxiliar configurada")
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

fn crear_esquema_sin_checks(ruta: &Path) -> Connection {
    let conn = abrir_segunda_conexion(ruta);
    conn.execute_batch(
        "CREATE TABLE secuencias (
            clave TEXT PRIMARY KEY,
            ultimo_valor INTEGER NOT NULL
        );
        CREATE TABLE ventanillas (
            id TEXT PRIMARY KEY,
            habilitada INTEGER NOT NULL,
            turno_actual_id TEXT NULL
        );
        CREATE TABLE turnos (
            id TEXT PRIMARY KEY,
            secuencia INTEGER NOT NULL,
            fecha_hora_registro TEXT NOT NULL,
            estado TEXT NOT NULL,
            ventanilla_asignada TEXT NULL
        );",
    )
    .expect("debe crear esquema flexible para pruebas de corrupción");
    conn
}

fn insertar_semillas_basicas(conn: &Connection) {
    conn.execute(
        "INSERT INTO secuencias (clave, ultimo_valor) VALUES ('turno', 0)",
        [],
    )
    .expect("debe insertar secuencia");
    for id in ["V01", "V02", "V03"] {
        conn.execute(
            "INSERT INTO ventanillas (id, habilitada, turno_actual_id) VALUES (?1, 1, NULL)",
            [id],
        )
        .expect("debe insertar ventanilla");
    }
}

#[test]
fn base_vacia_y_reapertura_no_duplican_semillas_ni_pierden_datos() {
    let dir = DirectorioPrueba::nuevo("vacia_reapertura");
    let ruta_anidada = dir.raiz.join("anidado").join("sgt.db");

    {
        let mut repo =
            RepositorioSqlite::abrir(&ruta_anidada).expect("debe crear directorio y BD nueva");
        let sistema = repo.cargar().expect("debe cargar estado inicial vacío");

        assert_eq!(sistema, SistemaTurnos::nuevo());
        assert_eq!(sistema.ultima_secuencia(), 0);
        assert!(sistema.consultar_fila().is_empty());
        assert!(sistema.consultar_historial().is_empty());
        assert!(ruta_anidada.exists());
    }

    let conn = abrir_segunda_conexion(&ruta_anidada);
    let journal_mode: String = conn
        .query_row("PRAGMA journal_mode;", [], |r| r.get(0))
        .expect("debe leer journal_mode");
    assert_eq!(journal_mode.to_ascii_lowercase(), "wal");

    let count_migraciones: i64 = conn
        .query_row("SELECT COUNT(*) FROM schema_migrations", [], |r| r.get(0))
        .expect("debe contar migraciones");
    let count_secuencias: i64 = conn
        .query_row("SELECT COUNT(*) FROM secuencias", [], |r| r.get(0))
        .expect("debe contar secuencias");
    let count_ventanillas: i64 = conn
        .query_row("SELECT COUNT(*) FROM ventanillas", [], |r| r.get(0))
        .expect("debe contar ventanillas");
    assert_eq!(count_migraciones, 1);
    assert_eq!(count_secuencias, 1);
    assert_eq!(count_ventanillas, 3);
    drop(conn);

    {
        let mut repo =
            RepositorioSqlite::abrir(&ruta_anidada).expect("debe reabrir BD existente sin error");
        let sistema = repo
            .cargar()
            .expect("debe cargar estado existente sin duplicar semillas");
        assert_eq!(sistema, SistemaTurnos::nuevo());
    }

    let conn = abrir_segunda_conexion(&ruta_anidada);
    let count_ventanillas_reabierta: i64 = conn
        .query_row("SELECT COUNT(*) FROM ventanillas", [], |r| r.get(0))
        .expect("debe contar ventanillas tras reapertura");
    assert_eq!(count_ventanillas_reabierta, 3);
    drop(conn);

    // Limpieza manual extra para la ruta anidada antes del Drop.
    for sufijo in ["", "-wal", "-shm"] {
        let _ = fs::remove_file(format!("{}{sufijo}", ruta_anidada.display()));
    }
}

#[test]
fn fallo_al_abrir_base_nueva_con_wal_bloqueado_no_deja_db_y_preserva_archivos_preexistentes() {
    // 1. Primera apertura bloqueada por directorio `sgt.db-wal`: falla con ErrorPersistencia
    //    sin dejar `sgt.db` creado; al retirar el bloqueo, reabrir crea esquema y semillas.
    {
        let dir = DirectorioPrueba::nuevo("wal_bloqueado_primera_apertura");
        fs::create_dir_all(&dir.raiz).expect("debe crear directorio de prueba");
        let wal_dir = dir.raiz.join("sgt.db-wal");
        fs::create_dir(&wal_dir).expect("debe crear directorio bloqueador sgt.db-wal");

        let res = RepositorioSqlite::abrir(dir.ruta());
        let err = res
            .err()
            .expect("la apertura debe fallar si sgt.db-wal es un directorio");
        assert_eq!(err.codigo, CodigoError::ErrorPersistencia);
        assert!(
            !dir.ruta().exists(),
            "no debe quedar un sgt.db parcial si falló la primera apertura"
        );

        fs::remove_dir(&wal_dir).expect("debe retirar directorio bloqueador sgt.db-wal");

        let mut repo = RepositorioSqlite::abrir(dir.ruta())
            .expect("al retirar el bloqueo debe abrir e inicializar la base nueva");
        let sistema = repo
            .cargar()
            .expect("debe cargar semillas iniciales tras retirar bloqueo");
        assert_eq!(sistema, SistemaTurnos::nuevo());
    }

    // 2. Archivo vacío preexistente: no se elimina ni se siembra silenciosamente.
    {
        let dir = DirectorioPrueba::nuevo("archivo_vacio_preexistente");
        fs::create_dir_all(&dir.raiz).expect("debe crear directorio de prueba");
        fs::write(dir.ruta(), b"").expect("debe crear archivo vacío preexistente");

        let wal_dir = dir.raiz.join("sgt.db-wal");
        fs::create_dir(&wal_dir).expect("debe crear directorio bloqueador sgt.db-wal");
        let err_bloqueo = RepositorioSqlite::abrir(dir.ruta())
            .err()
            .expect("abrir archivo vacío existente con wal bloqueado debe fallar al verificar WAL");
        assert_eq!(err_bloqueo.codigo, CodigoError::ErrorPersistencia);
        assert!(
            dir.ruta().exists(),
            "no debe eliminarse un archivo vacío preexistente"
        );
        fs::remove_dir(&wal_dir).expect("debe retirar directorio bloqueador");

        let mut repo = RepositorioSqlite::abrir(dir.ruta())
            .expect("abrir archivo vacío existente no lo borra ni re-inicializa");
        let err_cargar = repo
            .cargar()
            .expect_err("cargar archivo vacío preexistente no debe resembrar tablas");
        assert_eq!(err_cargar.codigo, CodigoError::BaseDatosCorrupta);
        drop(repo);

        let conn = abrir_segunda_conexion(dir.ruta());
        let tablas: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table'",
                [],
                |r| r.get(0),
            )
            .expect("debe consultar sqlite_master");
        assert_eq!(
            tablas, 0,
            "no debe crear tablas ni semillas sobre archivo preexistente"
        );
    }
    // 3. Archivo corrupto preexistente con wal bloqueado o sin bloqueo: se conserva intacto.
    {
        let dir = DirectorioPrueba::nuevo("archivo_corrupto_preexistente");
        fs::create_dir_all(&dir.raiz).expect("debe crear directorio de prueba");
        let bytes_corruptos = b"contenido corrupto preexistente";
        fs::write(dir.ruta(), bytes_corruptos).expect("debe escribir archivo corrupto");

        let wal_dir = dir.raiz.join("sgt.db-wal");
        fs::create_dir(&wal_dir).expect("debe crear directorio bloqueador sgt.db-wal");
        let err_bloqueo = RepositorioSqlite::abrir(dir.ruta())
            .err()
            .expect("abrir archivo corrupto con wal bloqueado debe fallar");
        assert!(matches!(
            err_bloqueo.codigo,
            CodigoError::BaseDatosCorrupta | CodigoError::ErrorPersistencia
        ));
        assert_eq!(
            fs::read(dir.ruta()).expect("debe conservar bytes corruptos"),
            bytes_corruptos
        );
        fs::remove_dir(&wal_dir).expect("debe retirar directorio bloqueador");

        let err_corrupto = RepositorioSqlite::abrir(dir.ruta())
            .err()
            .expect("abrir archivo corrupto preexistente debe fallar");
        assert_eq!(err_corrupto.codigo, CodigoError::BaseDatosCorrupta);
        assert_eq!(
            fs::read(dir.ruta()).expect("debe conservar archivo corrupto preexistente"),
            bytes_corruptos
        );
    }
}

#[test]
fn abrir_conexion_configurada_no_aplica_esquema_ni_repara_base_existente() {
    let dir = DirectorioPrueba::nuevo("configurada_sin_esquema");

    {
        let conn = db::abrir_conexion_configurada(dir.ruta())
            .expect("debe abrir archivo vacío configurado sin esquema");
        let tablas: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table'",
                [],
                |r| r.get(0),
            )
            .expect("debe consultar sqlite_master");
        assert_eq!(tablas, 0, "no debe aplicar tablas ni semillas");
    }

    // Un archivo SQLite existente sin tablas no se re-inicializa al abrir.
    let mut repo_sin_tablas = RepositorioSqlite::abrir(dir.ruta())
        .expect("abrir archivo SQLite existente sin tablas no falla ni repara");
    let err_sin_tablas = repo_sin_tablas
        .cargar()
        .expect_err("cargar sin tablas debe devolver BaseDatosCorrupta");
    assert_eq!(err_sin_tablas.codigo, CodigoError::BaseDatosCorrupta);

    let dir_existente = DirectorioPrueba::nuevo("no_reseed_existente");
    {
        let _repo =
            RepositorioSqlite::abrir(dir_existente.ruta()).expect("debe inicializar base nueva");
    }
    {
        let conn = abrir_segunda_conexion(dir_existente.ruta());
        conn.execute("DELETE FROM secuencias WHERE clave = 'turno'", [])
            .expect("debe eliminar fila de secuencia para simular daño");
    }

    let mut repo_reabierto = RepositorioSqlite::abrir(dir_existente.ruta())
        .expect("abrir archivo existente no falla ni repara semillas");
    let err = repo_reabierto
        .cargar()
        .expect_err("no debe reinsertar la fila ausente en secuencias");
    assert_eq!(err.codigo, CodigoError::BaseDatosCorrupta);
}

#[test]
fn registro_asignacion_finalizacion_cancelacion_y_ventanillas_persisten_exactos() {
    let dir = DirectorioPrueba::nuevo("ciclo_completo");
    let mut repo = RepositorioSqlite::abrir(dir.ruta()).expect("debe abrir repositorio");
    let mut sistema = repo.cargar().expect("debe cargar estado inicial");

    let t1 = sistema
        .registrar_turno_en(instante_fijo(1))
        .expect("debe registrar T-0001");
    let t2 = sistema
        .registrar_turno_en(instante_fijo(2))
        .expect("debe registrar T-0002");
    let t3 = sistema
        .registrar_turno_en(instante_fijo(3))
        .expect("debe registrar T-0003");
    repo.guardar(&Cambios {
        turnos: vec![t1, t2, t3],
        ventanillas: vec![],
        ultima_secuencia: sistema.ultima_secuencia(),
    })
    .expect("debe persistir tres turnos en espera");

    let t1_asignado = sistema
        .asignar_siguiente(VentanillaId::V02)
        .expect("debe asignar T-0001 a V02");
    repo.guardar(&Cambios {
        turnos: vec![t1_asignado],
        ventanillas: vec![sistema.ventanilla(VentanillaId::V02).clone()],
        ultima_secuencia: sistema.ultima_secuencia(),
    })
    .expect("debe persistir asignación en V02");

    let t1_atendido = sistema
        .finalizar_atencion(VentanillaId::V02)
        .expect("debe finalizar T-0001 en V02");
    repo.guardar(&Cambios {
        turnos: vec![t1_atendido],
        ventanillas: vec![sistema.ventanilla(VentanillaId::V02).clone()],
        ultima_secuencia: sistema.ultima_secuencia(),
    })
    .expect("debe persistir finalización en V02");

    let t2_cancelado = sistema
        .cancelar_turno("T-0002")
        .expect("debe cancelar T-0002");
    repo.guardar(&Cambios {
        turnos: vec![t2_cancelado],
        ventanillas: vec![],
        ultima_secuencia: sistema.ultima_secuencia(),
    })
    .expect("debe persistir cancelación de T-0002");

    // Inhabilitar V01 y V02; luego intercambiar V03->false y V01->true en el mismo Cambios
    // enviando primero la deshabilitación para probar el orden habilitada=true antes que false.
    let v01_off = sistema
        .cambiar_estado_ventanilla(VentanillaId::V01, false)
        .expect("debe inhabilitar V01");
    let v02_off = sistema
        .cambiar_estado_ventanilla(VentanillaId::V02, false)
        .expect("debe inhabilitar V02");
    repo.guardar(&Cambios {
        turnos: vec![],
        ventanillas: vec![v01_off, v02_off],
        ultima_secuencia: sistema.ultima_secuencia(),
    })
    .expect("debe inhabilitar V01 y V02 dejando V03 activa");

    let v01_on = sistema
        .cambiar_estado_ventanilla(VentanillaId::V01, true)
        .expect("debe habilitar V01");
    let v03_off = sistema
        .cambiar_estado_ventanilla(VentanillaId::V03, false)
        .expect("debe inhabilitar V03");
    repo.guardar(&Cambios {
        turnos: vec![],
        ventanillas: vec![v03_off, v01_on],
        ultima_secuencia: sistema.ultima_secuencia(),
    })
    .expect("debe aplicar habilitación antes de deshabilitación sin disparar trigger");

    // Asignar T-0003 en V01 para verificar también un turno EN_ATENCION al reabrir.
    let t3_asignado = sistema
        .asignar_siguiente(VentanillaId::V01)
        .expect("debe asignar T-0003 a V01");
    repo.guardar(&Cambios {
        turnos: vec![t3_asignado],
        ventanillas: vec![sistema.ventanilla(VentanillaId::V01).clone()],
        ultima_secuencia: sistema.ultima_secuencia(),
    })
    .expect("debe persistir T-0003 en atención");

    // Comparar directamente filas SQLite contra estado del dominio.
    let conn = abrir_segunda_conexion(dir.ruta());
    assert_eq!(leer_ultima_secuencia_sql(&conn), 3);
    assert_eq!(
        leer_ventanillas_sql(&conn),
        vec![
            ("V01".to_string(), 1, Some("T-0003".to_string())),
            ("V02".to_string(), 0, None),
            ("V03".to_string(), 0, None),
        ]
    );
    assert_eq!(
        leer_turnos_sql(&conn),
        vec![
            (
                "T-0001".to_string(),
                1,
                "2026-11-26T12:00:01.000Z".to_string(),
                "ATENDIDO".to_string(),
                Some("V02".to_string()),
            ),
            (
                "T-0002".to_string(),
                2,
                "2026-11-26T12:00:02.000Z".to_string(),
                "CANCELADO".to_string(),
                None,
            ),
            (
                "T-0003".to_string(),
                3,
                "2026-11-26T12:00:03.000Z".to_string(),
                "EN_ATENCION".to_string(),
                Some("V01".to_string()),
            ),
        ]
    );
    drop(conn);
    drop(repo);

    let mut repo_reabierto = RepositorioSqlite::abrir(dir.ruta()).expect("debe reabrir en disco");
    let restaurado = repo_reabierto
        .cargar()
        .expect("debe restaurar snapshot completo");
    assert_eq!(restaurado.ultima_secuencia(), sistema.ultima_secuencia());
    assert_eq!(restaurado.obtener_estado(), sistema.obtener_estado());
}

#[test]
fn guardar_lote_reemplaza_turno_en_atencion_en_misma_ventanilla_sin_colision_de_indice() {
    let dir = DirectorioPrueba::nuevo("lote_reemplazo_v01");
    let mut repo = RepositorioSqlite::abrir(dir.ruta()).expect("debe abrir BD");
    let mut sistema = repo.cargar().expect("debe cargar estado inicial");

    let t1 = sistema
        .registrar_turno_en(instante_fijo(1))
        .expect("debe registrar T-0001");
    let t2 = sistema
        .registrar_turno_en(instante_fijo(2))
        .expect("debe registrar T-0002");
    repo.guardar(&Cambios {
        turnos: vec![t1, t2],
        ventanillas: vec![],
        ultima_secuencia: sistema.ultima_secuencia(),
    })
    .expect("debe guardar T-0001 y T-0002");

    let t1_en_atencion = sistema
        .asignar_siguiente(VentanillaId::V01)
        .expect("debe asignar T-0001 a V01");
    repo.guardar(&Cambios {
        turnos: vec![t1_en_atencion],
        ventanillas: vec![sistema.ventanilla(VentanillaId::V01).clone()],
        ultima_secuencia: sistema.ultima_secuencia(),
    })
    .expect("debe persistir T-0001 en atención en V01");

    // En memoria: finalizar T-0001 en V01 y asignar T-0002 en V01.
    let t1_atendido = sistema
        .finalizar_atencion(VentanillaId::V01)
        .expect("debe finalizar T-0001 en V01");
    let t2_en_atencion = sistema
        .asignar_siguiente(VentanillaId::V01)
        .expect("debe asignar T-0002 a V01");

    // Enviar el lote con [T-0002 EN_ATENCION V01, T-0001 ATENDIDO V01]: debe escribir
    // primero los turnos no-EN_ATENCION para no violar ux_turnos_un_activo_por_ventanilla.
    let lote = Cambios {
        turnos: vec![
            Turno {
                estado: EstadoTurno::EnAtencion,
                ventanilla_asignada: Some(VentanillaId::V01),
                ..t2_en_atencion
            },
            Turno {
                estado: EstadoTurno::Atendido,
                ventanilla_asignada: Some(VentanillaId::V01),
                ..t1_atendido
            },
        ],
        ventanillas: vec![sistema.ventanilla(VentanillaId::V01).clone()],
        ultima_secuencia: sistema.ultima_secuencia(),
    };
    repo.guardar(&lote)
        .expect("el lote [T-0002 EN_ATENCION V01, T-0001 ATENDIDO V01] debe guardarse sin error");

    let conn = abrir_segunda_conexion(dir.ruta());
    assert_eq!(
        leer_ventanillas_sql(&conn)[0],
        ("V01".to_string(), 1, Some("T-0002".to_string()))
    );
    assert_eq!(
        leer_turnos_sql(&conn),
        vec![
            (
                "T-0001".to_string(),
                1,
                "2026-11-26T12:00:01.000Z".to_string(),
                "ATENDIDO".to_string(),
                Some("V01".to_string()),
            ),
            (
                "T-0002".to_string(),
                2,
                "2026-11-26T12:00:02.000Z".to_string(),
                "EN_ATENCION".to_string(),
                Some("V01".to_string()),
            ),
        ]
    );
    drop(conn);
    drop(repo);

    let mut reabierto = RepositorioSqlite::abrir(dir.ruta()).expect("debe reabrir BD");
    let restaurado = reabierto.cargar().expect("debe cargar lote confirmado");
    assert_eq!(restaurado.obtener_estado(), sistema.obtener_estado());
}

#[test]
fn reinicio_no_reutiliza_secuencias_y_conserva_contador_mayor_que_max_secuencia() {
    let dir = DirectorioPrueba::nuevo("reinicio_secuencia");

    {
        let mut repo = RepositorioSqlite::abrir(dir.ruta()).expect("debe abrir BD");
        let mut sistema = repo.cargar().expect("debe cargar inicial");
        let t1 = sistema
            .registrar_turno_en(instante_fijo(10))
            .expect("T-0001");
        let t2 = sistema
            .registrar_turno_en(instante_fijo(11))
            .expect("T-0002");
        repo.guardar(&Cambios {
            turnos: vec![t1, t2],
            ventanillas: vec![],
            ultima_secuencia: 2,
        })
        .expect("debe guardar T-0001 y T-0002");
    }

    {
        let mut repo = RepositorioSqlite::abrir(dir.ruta()).expect("debe reabrir BD");
        let mut sistema = repo.cargar().expect("debe cargar con contador 2");
        assert_eq!(sistema.ultima_secuencia(), 2);

        let t3 = sistema
            .registrar_turno_en(instante_fijo(12))
            .expect("debe generar T-0003 sin reutilizar IDs");
        assert_eq!(t3.id, "T-0003");
        assert_eq!(t3.secuencia, 3);

        // Guardar también un contador superior a MAX(secuencia) (ej. saltos o huecos).
        repo.guardar(&Cambios {
            turnos: vec![t3],
            ventanillas: vec![],
            ultima_secuencia: 10,
        })
        .expect("debe conservar ultima_secuencia=10 mayor que MAX(secuencia)=3");
    }

    {
        let mut repo =
            RepositorioSqlite::abrir(dir.ruta()).expect("debe reabrir BD con contador 10");
        let mut sistema = repo.cargar().expect("debe cargar contador 10 exacto");
        assert_eq!(sistema.ultima_secuencia(), 10);
        assert_eq!(sistema.consultar_historial().len(), 3);

        // Una operación que no registra turnos preserva el contador sin incrementarlo.
        let v01_off = sistema
            .cambiar_estado_ventanilla(VentanillaId::V01, false)
            .expect("debe inhabilitar V01");
        repo.guardar(&Cambios {
            turnos: vec![],
            ventanillas: vec![v01_off],
            ultima_secuencia: sistema.ultima_secuencia(),
        })
        .expect("debe conservar contador=10 en operación sin registro");
        assert_eq!(sistema.ultima_secuencia(), 10);

        let t11 = sistema
            .registrar_turno_en(instante_fijo(13))
            .expect("debe asignar secuencia 11");
        assert_eq!(t11.id, "T-0011");
        assert_eq!(t11.secuencia, 11);
        repo.guardar(&Cambios {
            turnos: vec![t11],
            ventanillas: vec![],
            ultima_secuencia: sistema.ultima_secuencia(),
        })
        .expect("debe guardar T-0011");
    }

    let mut repo_final = RepositorioSqlite::abrir(dir.ruta()).expect("debe reabrir final");
    let sistema_final = repo_final.cargar().expect("debe cargar final");
    assert_eq!(sistema_final.ultima_secuencia(), 11);
    assert_eq!(
        sistema_final
            .consultar_historial()
            .iter()
            .map(|t| t.id.as_str())
            .collect::<Vec<_>>(),
        vec!["T-0001", "T-0002", "T-0003", "T-0011"]
    );
}

#[test]
fn operaciones_posteriores_preservan_historial_terminal_y_rechazan_modificarlo() {
    let dir = DirectorioPrueba::nuevo("historial_terminal");
    let mut repo = RepositorioSqlite::abrir(dir.ruta()).expect("debe abrir BD");
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

    let t1_en_atencion = sistema
        .asignar_siguiente(VentanillaId::V01)
        .expect("asignar T-0001");
    repo.guardar(&Cambios {
        turnos: vec![t1_en_atencion],
        ventanillas: vec![sistema.ventanilla(VentanillaId::V01).clone()],
        ultima_secuencia: 3,
    })
    .expect("debe guardar T-0001 en atención");

    let t1_atendido = sistema
        .finalizar_atencion(VentanillaId::V01)
        .expect("finalizar T-0001");
    let t2_cancelado = sistema.cancelar_turno("T-0002").expect("cancelar T-0002");
    repo.guardar(&Cambios {
        turnos: vec![t1_atendido.clone(), t2_cancelado.clone()],
        ventanillas: vec![sistema.ventanilla(VentanillaId::V01).clone()],
        ultima_secuencia: 3,
    })
    .expect("debe guardar estados terminales");

    // Operación posterior: guardar solo T-0003 y T-0004 (sin incluir T-0001/T-0002).
    let t3_asignado = sistema
        .asignar_siguiente(VentanillaId::V02)
        .expect("asignar T-0003");
    let t4 = sistema
        .registrar_turno_en(instante_fijo(4))
        .expect("registrar T-0004");
    repo.guardar(&Cambios {
        turnos: vec![t3_asignado, t4],
        ventanillas: vec![sistema.ventanilla(VentanillaId::V02).clone()],
        ultima_secuencia: 4,
    })
    .expect("no debe intentar actualizar turnos terminales existentes");

    // Si alguien intenta reenviar una fila terminal en Cambios.turnos, el trigger la bloquea.
    let err = repo
        .guardar(&Cambios {
            turnos: vec![t1_atendido],
            ventanillas: vec![],
            ultima_secuencia: 4,
        })
        .expect_err("el trigger trg_turnos_estados_terminales debe rechazar update terminal");
    assert_eq!(err.codigo, CodigoError::ErrorPersistencia);

    let err_cancelado = repo
        .guardar(&Cambios {
            turnos: vec![t2_cancelado],
            ventanillas: vec![],
            ultima_secuencia: 4,
        })
        .expect_err("el trigger debe rechazar también fila CANCELADO existente");
    assert_eq!(err_cancelado.codigo, CodigoError::ErrorPersistencia);

    let restaurado = repo
        .cargar()
        .expect("conexión sigue operativa y consistente");
    assert_eq!(restaurado.ultima_secuencia(), 4);
    assert_eq!(
        restaurado
            .buscar_turno("T-0001")
            .expect("T-0001 existe")
            .estado,
        EstadoTurno::Atendido
    );
    assert_eq!(
        restaurado
            .buscar_turno("T-0002")
            .expect("T-0002 existe")
            .estado,
        EstadoTurno::Cancelado
    );
}

#[test]
fn fallo_sql_a_mitad_de_escritura_revierte_todo_y_permite_reutilizar_conexion() {
    let dir = DirectorioPrueba::nuevo("rollback_mitad_escritura");
    let mut repo = RepositorioSqlite::abrir(dir.ruta()).expect("debe abrir BD");
    let mut sistema = repo.cargar().expect("debe cargar estado inicial");

    let t1 = sistema
        .registrar_turno_en(instante_fijo(1))
        .expect("debe registrar T-0001");
    repo.guardar(&Cambios {
        turnos: vec![t1],
        ventanillas: vec![],
        ultima_secuencia: 1,
    })
    .expect("debe persistir T-0001");

    // Preparar un borrador con T-0002 nuevo y asignación de T-0001 en V01.
    let mut borrador = sistema.clone();
    let t2 = borrador
        .registrar_turno_en(instante_fijo(2))
        .expect("debe registrar T-0002 en borrador");
    let t1_asignado = borrador
        .asignar_siguiente(VentanillaId::V01)
        .expect("debe asignar T-0001 en borrador");
    let cambios = Cambios {
        turnos: vec![t1_asignado, t2],
        ventanillas: vec![borrador.ventanilla(VentanillaId::V01).clone()],
        ultima_secuencia: borrador.ultima_secuencia(),
    };

    // Caso A: fallo después de escribir `turnos`, justo al actualizar `ventanillas`.
    let conn_aux = abrir_segunda_conexion(dir.ruta());
    conn_aux
        .execute_batch(
            "CREATE TRIGGER trg_test_fallo_ventanillas
             BEFORE UPDATE ON ventanillas
             BEGIN
                 SELECT RAISE(ABORT, 'FALLO_SIMULADO_VENTANILLAS');
             END;",
        )
        .expect("debe instalar trigger de prueba en ventanillas");

    let err = repo
        .guardar(&cambios)
        .expect_err("el trigger debe abortar tras insertar/actualizar turnos");
    assert_eq!(err.codigo, CodigoError::ErrorPersistencia);

    // Verificar desde la segunda conexión (con repo aún abierto) que nada quedó parcial.
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
            "2026-11-26T12:00:01.000Z".to_string(),
            "EN_ESPERA".to_string(),
            None,
        )]
    );

    // Retirar trigger de ventanillas e instalar uno en secuencias (después de turnos y ventanillas).
    conn_aux
        .execute_batch(
            "DROP TRIGGER trg_test_fallo_ventanillas;
             CREATE TRIGGER trg_test_fallo_secuencias
             BEFORE UPDATE ON secuencias
             BEGIN
                 SELECT RAISE(ABORT, 'FALLO_SIMULADO_SECUENCIAS');
             END;",
        )
        .expect("debe instalar trigger de prueba en secuencias");

    let err_sec = repo
        .guardar(&cambios)
        .expect_err("el trigger debe abortar al actualizar secuencias");
    assert_eq!(err_sec.codigo, CodigoError::ErrorPersistencia);

    assert_eq!(leer_ultima_secuencia_sql(&conn_aux), 1);
    assert_eq!(
        leer_ventanillas_sql(&conn_aux)[0],
        ("V01".to_string(), 1, None)
    );
    assert_eq!(leer_turnos_sql(&conn_aux).len(), 1);

    // Retirar la causa del fallo y confirmar en la MISMA instancia de RepositorioSqlite.
    conn_aux
        .execute_batch("DROP TRIGGER trg_test_fallo_secuencias;")
        .expect("debe retirar trigger de prueba");
    drop(conn_aux);

    repo.guardar(&cambios)
        .expect("la misma conexión debe reutilizarse tras rollback sin transacción pendiente");

    let cargado = repo
        .cargar()
        .expect("debe cargar cambios confirmados tras reutilizar conexión");
    assert_eq!(cargado.ultima_secuencia(), 2);
    assert_eq!(cargado.obtener_estado(), borrador.obtener_estado());
    drop(repo);

    let mut reabierto = RepositorioSqlite::abrir(dir.ruta()).expect("debe reabrir en disco");
    let tras_reabrir = reabierto.cargar().expect("debe cargar tras reabrir");
    assert_eq!(tras_reabrir.ultima_secuencia(), 2);
    assert_eq!(tras_reabrir.obtener_estado(), borrador.obtener_estado());
}

#[test]
fn bloqueo_de_escritura_falla_controladamente_sin_cambios_parciales() {
    let dir = DirectorioPrueba::nuevo("bloqueo_escritura");
    let mut repo = RepositorioSqlite::abrir(dir.ruta()).expect("debe abrir BD");
    let mut sistema = repo.cargar().expect("debe cargar inicial");

    let t1 = sistema
        .registrar_turno_en(instante_fijo(1))
        .expect("debe registrar T-0001");
    repo.guardar(&Cambios {
        turnos: vec![t1],
        ventanillas: vec![],
        ultima_secuencia: 1,
    })
    .expect("debe guardar T-0001");

    let mut borrador = sistema.clone();
    let t2 = borrador
        .registrar_turno_en(instante_fijo(2))
        .expect("debe registrar T-0002 en borrador");
    let cambios = Cambios {
        turnos: vec![t2],
        ventanillas: vec![],
        ultima_secuencia: 2,
    };

    // Otra conexión retiene el bloqueo inmediato de escritura hasta vencer busy_timeout.
    let conn_bloqueo = abrir_segunda_conexion(dir.ruta());
    conn_bloqueo
        .execute_batch("BEGIN IMMEDIATE;")
        .expect("debe adquirir bloqueo de escritura en conexión auxiliar");

    let err = repo
        .guardar(&cambios)
        .expect_err("el intento concurrente debe fallar al vencer busy_timeout");
    assert_eq!(err.codigo, CodigoError::ErrorPersistencia);

    conn_bloqueo
        .execute_batch("ROLLBACK;")
        .expect("debe liberar bloqueo de escritura");
    assert_eq!(leer_ultima_secuencia_sql(&conn_bloqueo), 1);
    assert_eq!(leer_turnos_sql(&conn_bloqueo).len(), 1);
    drop(conn_bloqueo);

    // Tras liberar el bloqueo, el estado sigue intacto y el repositorio continúa usable.
    let intacto = repo.cargar().expect("debe cargar estado intacto");
    assert_eq!(intacto.ultima_secuencia(), 1);
    assert_eq!(intacto.consultar_historial().len(), 1);

    repo.guardar(&cambios)
        .expect("debe confirmar guardado una vez liberado el bloqueo");
    assert_eq!(
        repo.cargar()
            .expect("debe cargar estado actualizado")
            .ultima_secuencia(),
        2
    );
}

#[test]
fn guardar_rechaza_secuencias_invalidas_y_restricciones_sin_alterar_datos() {
    let dir = DirectorioPrueba::nuevo("errores_guardar");
    let mut repo = RepositorioSqlite::abrir(dir.ruta()).expect("debe abrir BD");
    let mut sistema = repo.cargar().expect("debe cargar inicial");

    let t1 = sistema
        .registrar_turno_en(instante_fijo(1))
        .expect("T-0001");
    let t2 = sistema
        .registrar_turno_en(instante_fijo(2))
        .expect("T-0002");
    repo.guardar(&Cambios {
        turnos: vec![t1, t2.clone()],
        ventanillas: vec![],
        ultima_secuencia: 2,
    })
    .expect("debe persistir estado base");

    // 1. Contador decreciente.
    let err_dec = repo
        .guardar(&Cambios {
            turnos: vec![],
            ventanillas: vec![],
            ultima_secuencia: 1,
        })
        .expect_err("debe rechazar secuencia decreciente");
    assert_eq!(err_dec.codigo, CodigoError::ErrorPersistencia);

    // 2. Contador fuera de MAX_SECUENCIA_JS.
    let err_max = repo
        .guardar(&Cambios {
            turnos: vec![],
            ventanillas: vec![],
            ultima_secuencia: MAX_SECUENCIA_JS + 1,
        })
        .expect_err("debe rechazar ultima_secuencia > MAX_SECUENCIA_JS");
    assert_eq!(err_max.codigo, CodigoError::ErrorPersistencia);

    // 3. Turno con secuencia 0.
    let mut turno_cero = t2.clone();
    turno_cero.id = "T-0000".to_string();
    turno_cero.secuencia = 0;
    let err_cero = repo
        .guardar(&Cambios {
            turnos: vec![turno_cero],
            ventanillas: vec![],
            ultima_secuencia: 2,
        })
        .expect_err("debe rechazar turno con secuencia 0");
    assert_eq!(err_cero.codigo, CodigoError::ErrorPersistencia);

    // 4. Turno con secuencia mayor a ultima_secuencia del propio Cambios.
    let mut turno_adelantado = t2.clone();
    turno_adelantado.id = "T-0003".to_string();
    turno_adelantado.secuencia = 3;
    let err_adelantado = repo
        .guardar(&Cambios {
            turnos: vec![turno_adelantado],
            ventanillas: vec![],
            ultima_secuencia: 2,
        })
        .expect_err("debe rechazar turno con secuencia > ultima_secuencia");
    assert_eq!(err_adelantado.codigo, CodigoError::ErrorPersistencia);

    // 5. Turno con secuencia > MAX_SECUENCIA_JS.
    let mut turno_overflow = t2;
    turno_overflow.id = "T-MAX".to_string();
    turno_overflow.secuencia = MAX_SECUENCIA_JS + 1;
    let err_turno_overflow = repo
        .guardar(&Cambios {
            turnos: vec![turno_overflow],
            ventanillas: vec![],
            ultima_secuencia: MAX_SECUENCIA_JS + 1,
        })
        .expect_err("debe rechazar turno con secuencia > MAX_SECUENCIA_JS");
    assert_eq!(err_turno_overflow.codigo, CodigoError::ErrorPersistencia);

    // 6. Trigger de última ventanilla activa al intentar inhabilitar las 3.
    let err_ultima = repo
        .guardar(&Cambios {
            turnos: vec![],
            ventanillas: [VentanillaId::V01, VentanillaId::V02, VentanillaId::V03]
                .into_iter()
                .map(|id| Ventanilla {
                    id,
                    habilitada: false,
                    turno_actual_id: None,
                })
                .collect(),
            ultima_secuencia: 2,
        })
        .expect_err("el trigger debe impedir inhabilitar todas las ventanillas");
    assert_eq!(err_ultima.codigo, CodigoError::ErrorPersistencia);

    // 7. CHECK de ventanilla inhabilitada con turno asignado.
    let err_ocupada = repo
        .guardar(&Cambios {
            turnos: vec![],
            ventanillas: vec![Ventanilla {
                id: VentanillaId::V01,
                habilitada: false,
                turno_actual_id: Some("T-0001".to_string()),
            }],
            ultima_secuencia: 2,
        })
        .expect_err("el CHECK debe rechazar ventanilla inhabilitada con turno asignado");
    assert_eq!(err_ocupada.codigo, CodigoError::ErrorPersistencia);

    // Verificar que ningún intento fallido alteró el estado y que el repo sigue usable.
    let cargado = repo.cargar().expect("debe cargar intacto");
    assert_eq!(cargado.ultima_secuencia(), 2);
    assert_eq!(cargado.obtener_estado(), sistema.obtener_estado());
}

#[test]
fn cargar_rechaza_datos_incompatibles_conversiones_y_archivo_corrupto() {
    // 1. Archivo existente no SQLite: devuelve BaseDatosCorrupta sin borrar ni recrear el archivo.
    {
        let dir = DirectorioPrueba::nuevo("archivo_no_sqlite");
        fs::create_dir_all(&dir.raiz).expect("debe crear directorio");
        let contenido_original = b"esto no es una base sqlite valida";
        fs::write(dir.ruta(), contenido_original).expect("debe escribir bytes inválidos");

        let res = RepositorioSqlite::abrir(dir.ruta());
        assert!(res.is_err(), "abrir un archivo no SQLite debe fallar");
        let err = res.err().expect("error esperado");
        assert_eq!(err.codigo, CodigoError::BaseDatosCorrupta);
        assert_eq!(
            fs::read(dir.ruta()).expect("debe leer archivo"),
            contenido_original,
            "un archivo existente no debe borrarse ni sobreescribirse ante error"
        );
    }

    // 2. Ruta cuyo padre es un archivo regular: falla controladamente.
    {
        let dir = DirectorioPrueba::nuevo("padre_archivo");
        fs::create_dir_all(&dir.raiz).expect("debe crear directorio");
        let padre_archivo = dir.raiz.join("bloqueador");
        fs::write(&padre_archivo, b"x").expect("debe crear archivo");
        let ruta_invalida = padre_archivo.join("sgt.db");

        let err = RepositorioSqlite::abrir(&ruta_invalida)
            .err()
            .expect("abrir bajo un archivo regular debe fallar");
        assert_eq!(err.codigo, CodigoError::ErrorPersistencia);
    }

    // 3. Ventanilla con ID desconocido ('V04').
    {
        let dir = DirectorioPrueba::nuevo("ventanilla_desconocida");
        let conn = crear_esquema_sin_checks(dir.ruta());
        conn.execute(
            "INSERT INTO secuencias (clave, ultimo_valor) VALUES ('turno', 0)",
            [],
        )
        .expect("secuencia");
        for id in ["V01", "V02", "V04"] {
            conn.execute(
                "INSERT INTO ventanillas (id, habilitada, turno_actual_id) VALUES (?1, 1, NULL)",
                [id],
            )
            .expect("ventanilla");
        }
        drop(conn);

        let mut repo = RepositorioSqlite::abrir(dir.ruta()).expect("abre archivo existente");
        let err = repo.cargar().expect_err("V04 debe rechazarse");
        assert_eq!(err.codigo, CodigoError::BaseDatosCorrupta);
    }

    // 4. Ventanilla con valor `habilitada` distinto de 0/1.
    {
        let dir = DirectorioPrueba::nuevo("habilitada_invalida");
        let conn = crear_esquema_sin_checks(dir.ruta());
        insertar_semillas_basicas(&conn);
        conn.execute("UPDATE ventanillas SET habilitada = 2 WHERE id = 'V01'", [])
            .expect("habilitada=2");
        drop(conn);

        let mut repo = RepositorioSqlite::abrir(dir.ruta()).expect("abre archivo existente");
        let err = repo.cargar().expect_err("habilitada=2 debe rechazarse");
        assert_eq!(err.codigo, CodigoError::BaseDatosCorrupta);
    }

    // 5. Cantidad de ventanillas distinta de 3.
    {
        let dir = DirectorioPrueba::nuevo("ventanillas_incompletas");
        let conn = crear_esquema_sin_checks(dir.ruta());
        insertar_semillas_basicas(&conn);
        conn.execute("DELETE FROM ventanillas WHERE id = 'V03'", [])
            .expect("elimina V03");
        drop(conn);

        let mut repo = RepositorioSqlite::abrir(dir.ruta()).expect("abre archivo existente");
        let err = repo
            .cargar()
            .expect_err("menos de 3 ventanillas debe rechazarse");
        assert_eq!(err.codigo, CodigoError::BaseDatosCorrupta);
    }

    // 6. Estado de turno desconocido.
    {
        let dir = DirectorioPrueba::nuevo("estado_desconocido");
        let conn = crear_esquema_sin_checks(dir.ruta());
        insertar_semillas_basicas(&conn);
        conn.execute("UPDATE secuencias SET ultimo_valor = 1", [])
            .expect("seq=1");
        conn.execute(
            "INSERT INTO turnos (id, secuencia, fecha_hora_registro, estado, ventanilla_asignada)
             VALUES ('T-0001', 1, '2026-11-26T12:00:00Z', 'PAUSADO', NULL)",
            [],
        )
        .expect("turno estado desconocido");
        drop(conn);

        let mut repo = RepositorioSqlite::abrir(dir.ruta()).expect("abre archivo existente");
        let err = repo.cargar().expect_err("estado PAUSADO debe rechazarse");
        assert_eq!(err.codigo, CodigoError::BaseDatosCorrupta);
    }

    // 7. Turno con ventanilla_asignada desconocida.
    {
        let dir = DirectorioPrueba::nuevo("turno_ventanilla_desconocida");
        let conn = crear_esquema_sin_checks(dir.ruta());
        insertar_semillas_basicas(&conn);
        conn.execute("UPDATE secuencias SET ultimo_valor = 1", [])
            .expect("seq=1");
        conn.execute(
            "INSERT INTO turnos (id, secuencia, fecha_hora_registro, estado, ventanilla_asignada)
             VALUES ('T-0001', 1, '2026-11-26T12:00:00Z', 'ATENDIDO', 'V04')",
            [],
        )
        .expect("turno V04");
        drop(conn);

        let mut repo = RepositorioSqlite::abrir(dir.ruta()).expect("abre archivo existente");
        let err = repo.cargar().expect_err("V04 en turno debe rechazarse");
        assert_eq!(err.codigo, CodigoError::BaseDatosCorrupta);
    }

    // 8. Secuencia negativa en `secuencias`.
    {
        let dir = DirectorioPrueba::nuevo("seq_negativa");
        let conn = crear_esquema_sin_checks(dir.ruta());
        insertar_semillas_basicas(&conn);
        conn.execute("UPDATE secuencias SET ultimo_valor = -1", [])
            .expect("seq=-1");
        drop(conn);

        let mut repo = RepositorioSqlite::abrir(dir.ruta()).expect("abre archivo existente");
        let err = repo.cargar().expect_err("secuencia negativa debe fallar");
        assert_eq!(err.codigo, CodigoError::BaseDatosCorrupta);
    }

    // 9. Secuencia fuera de rango (> MAX_SECUENCIA_JS).
    {
        let dir = DirectorioPrueba::nuevo("seq_overflow_js");
        let conn = crear_esquema_sin_checks(dir.ruta());
        insertar_semillas_basicas(&conn);
        let fuera_rango = i64::try_from(MAX_SECUENCIA_JS + 1).expect("cabe en i64");
        conn.execute("UPDATE secuencias SET ultimo_valor = ?1", [fuera_rango])
            .expect("seq > MAX_SECUENCIA_JS");
        drop(conn);

        let mut repo = RepositorioSqlite::abrir(dir.ruta()).expect("abre archivo existente");
        let err = repo
            .cargar()
            .expect_err("secuencia > MAX_SECUENCIA_JS debe rechazarse sin truncamiento");
        assert_eq!(err.codigo, CodigoError::BaseDatosCorrupta);
    }

    // 10. Tipo incompatible en columna numérica (TEXT en `secuencias.ultimo_valor`)
    //     y TEXT con UTF-8 inválido (`CAST(X'80' AS TEXT)`).
    {
        let dir = DirectorioPrueba::nuevo("seq_tipo_texto");
        let conn = crear_esquema_sin_checks(dir.ruta());
        insertar_semillas_basicas(&conn);
        conn.execute("UPDATE secuencias SET ultimo_valor = 'no_numero'", [])
            .expect("texto en ultimo_valor");
        drop(conn);

        let mut repo = RepositorioSqlite::abrir(dir.ruta()).expect("abre archivo existente");
        let err = repo
            .cargar()
            .expect_err("tipo no entero en secuencia debe rechazarse");
        assert_eq!(err.codigo, CodigoError::BaseDatosCorrupta);
    }

    {
        let dir = DirectorioPrueba::nuevo("text_utf8_invalido");
        let conn = crear_esquema_sin_checks(dir.ruta());
        insertar_semillas_basicas(&conn);
        conn.execute("UPDATE secuencias SET ultimo_valor = 1", [])
            .expect("seq=1");
        conn.execute(
            "INSERT INTO turnos (id, secuencia, fecha_hora_registro, estado, ventanilla_asignada)
             VALUES ('T-0001', 1, CAST(X'80' AS TEXT), 'EN_ESPERA', NULL)",
            [],
        )
        .expect("inserta TEXT con bytes UTF-8 inválidos");
        drop(conn);

        let mut repo = RepositorioSqlite::abrir(dir.ruta()).expect("abre archivo existente");
        let err = repo
            .cargar()
            .expect_err("TEXT con UTF-8 inválido debe cargarse como BaseDatosCorrupta");
        assert_eq!(err.codigo, CodigoError::BaseDatosCorrupta);
    }

    // 11. Turno con secuencia > ultimo_valor (inconsistencia de contador) y secuencia negativa.
    {
        let dir = DirectorioPrueba::nuevo("turno_mayor_que_contador");
        let conn = crear_esquema_sin_checks(dir.ruta());
        insertar_semillas_basicas(&conn);
        conn.execute("UPDATE secuencias SET ultimo_valor = 1", [])
            .expect("seq=1");
        conn.execute(
            "INSERT INTO turnos (id, secuencia, fecha_hora_registro, estado, ventanilla_asignada)
             VALUES ('T-0002', 2, '2026-11-26T12:00:00Z', 'EN_ESPERA', NULL)",
            [],
        )
        .expect("turno seq=2 > ultimo_valor=1");
        drop(conn);

        let mut repo = RepositorioSqlite::abrir(dir.ruta()).expect("abre archivo existente");
        let err = repo
            .cargar()
            .expect_err("turno con secuencia > ultimo_valor debe rechazarse");
        assert_eq!(err.codigo, CodigoError::BaseDatosCorrupta);
    }

    {
        let dir = DirectorioPrueba::nuevo("turno_seq_negativa");
        let conn = crear_esquema_sin_checks(dir.ruta());
        insertar_semillas_basicas(&conn);
        conn.execute("UPDATE secuencias SET ultimo_valor = 1", [])
            .expect("seq=1");
        conn.execute(
            "INSERT INTO turnos (id, secuencia, fecha_hora_registro, estado, ventanilla_asignada)
             VALUES ('T-0001', -5, '2026-11-26T12:00:00Z', 'EN_ESPERA', NULL)",
            [],
        )
        .expect("turno seq=-5");
        drop(conn);

        let mut repo = RepositorioSqlite::abrir(dir.ruta()).expect("abre archivo existente");
        let err = repo
            .cargar()
            .expect_err("turno con secuencia negativa debe rechazarse");
        assert_eq!(err.codigo, CodigoError::BaseDatosCorrupta);
    }

    // 12. Inconsistencias estructurales detectadas por `SistemaTurnos::restaurar`.
    {
        let dir = DirectorioPrueba::nuevo("todas_inhabilitadas");
        let conn = crear_esquema_sin_checks(dir.ruta());
        insertar_semillas_basicas(&conn);
        conn.execute("UPDATE ventanillas SET habilitada = 0", [])
            .expect("todas inhabilitadas");
        drop(conn);

        let mut repo = RepositorioSqlite::abrir(dir.ruta()).expect("abre archivo existente");
        let err = repo
            .cargar()
            .expect_err("cero ventanillas habilitadas debe rechazarse en restaurar");
        assert_eq!(err.codigo, CodigoError::BaseDatosCorrupta);
    }

    {
        let dir = DirectorioPrueba::nuevo("en_atencion_sin_ventanilla_coincidente");
        let conn = crear_esquema_sin_checks(dir.ruta());
        insertar_semillas_basicas(&conn);
        conn.execute("UPDATE secuencias SET ultimo_valor = 1", [])
            .expect("seq=1");
        conn.execute(
            "INSERT INTO turnos (id, secuencia, fecha_hora_registro, estado, ventanilla_asignada)
             VALUES ('T-0001', 1, '2026-11-26T12:00:00Z', 'EN_ATENCION', 'V01')",
            [],
        )
        .expect("turno EN_ATENCION sin enlace en V01");
        drop(conn);

        let mut repo = RepositorioSqlite::abrir(dir.ruta()).expect("abre archivo existente");
        let err = repo
            .cargar()
            .expect_err("desajuste EN_ATENCION vs ventanilla debe rechazarse");
        assert_eq!(err.codigo, CodigoError::BaseDatosCorrupta);
    }
}

#[test]
fn preparar_temporal_omite_auxiliares_huerfanos_y_no_los_toca() {
    let pid = std::process::id();

    // 1. Auxiliares hu?rfanos `-wal`: no se abren, truncan ni eliminan; se omiten.
    {
        let dir = DirectorioPrueba::nuevo("huerfanos_tmp_wal");
        fs::create_dir_all(&dir.raiz).expect("debe crear directorio de prueba");

        let rango_wal = 0..64u64;
        let mut esperados = Vec::new();
        for n in rango_wal.clone() {
            let nombre = format!("sgt.db.tmp-{pid}-{n}-wal");
            let ruta = dir.raiz.join(&nombre);
            let contenido = format!("wal-huerfano-{n}").into_bytes();
            fs::write(&ruta, &contenido).expect("debe crear -wal hu?rfano");
            esperados.push((ruta, contenido));
        }

        let mut repo = RepositorioSqlite::abrir(dir.ruta())
            .expect("debe omitir auxiliares -wal hu?rfanos y abrir la BD nueva");
        let sistema = repo
            .cargar()
            .expect("debe cargar estado inicial v?lido tras omitir hu?rfanos");
        assert_eq!(sistema, SistemaTurnos::nuevo());
        drop(repo);

        for (ruta, contenido) in esperados {
            assert!(
                ruta.exists(),
                "el auxiliar -wal hu?rfano '{}' no debe eliminarse",
                ruta.display()
            );
            assert_eq!(
                fs::read(&ruta).expect("debe leer -wal hu?rfano"),
                contenido,
                "el auxiliar -wal hu?rfano '{}' no debe modificarse",
                ruta.display()
            );
        }

        for entrada in fs::read_dir(&dir.raiz).expect("debe listar directorio") {
            let nombre = entrada
                .expect("entrada v?lida")
                .file_name()
                .to_string_lossy()
                .into_owned();
            if nombre.contains(".tmp-") {
                assert!(
                    nombre.ends_with("-wal"),
                    "no deben quedar archivos temporales principales residuales: {nombre}"
                );
            }
        }
    }

    // 2. Auxiliares hu?rfanos `-shm`: tampoco se tocan ni reutilizan.
    {
        let dir = DirectorioPrueba::nuevo("huerfanos_tmp_shm");
        fs::create_dir_all(&dir.raiz).expect("debe crear directorio de prueba");

        let rango_shm = 0..160u64;
        let mut esperados = Vec::new();
        for n in rango_shm.clone() {
            let nombre = format!("sgt.db.tmp-{pid}-{n}-shm");
            let ruta = dir.raiz.join(&nombre);
            let contenido = format!("shm-huerfano-{n}").into_bytes();
            fs::write(&ruta, &contenido).expect("debe crear -shm hu?rfano");
            esperados.push((ruta, contenido));
        }

        let mut repo = RepositorioSqlite::abrir(dir.ruta())
            .expect("debe omitir auxiliares -shm hu?rfanos y abrir la BD nueva");
        let sistema = repo
            .cargar()
            .expect("debe cargar estado inicial v?lido tras omitir -shm hu?rfanos");
        assert_eq!(sistema, SistemaTurnos::nuevo());
        drop(repo);

        for (ruta, contenido) in esperados {
            assert!(
                ruta.exists(),
                "el auxiliar -shm hu?rfano '{}' no debe eliminarse",
                ruta.display()
            );
            assert_eq!(
                fs::read(&ruta).expect("debe leer -shm hu?rfano"),
                contenido,
                "el auxiliar -shm hu?rfano '{}' no debe modificarse",
                ruta.display()
            );
        }

        for entrada in fs::read_dir(&dir.raiz).expect("debe listar directorio") {
            let nombre = entrada
                .expect("entrada v?lida")
                .file_name()
                .to_string_lossy()
                .into_owned();
            if nombre.contains(".tmp-") {
                assert!(
                    nombre.ends_with("-shm"),
                    "no deben quedar archivos temporales principales residuales: {nombre}"
                );
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ModoFalloVfs {
    FalloTruncateWalEnCheckpoint,
    FalloExclusiveLockEnTransicionDelete,
}

#[derive(Clone)]
struct ReglaVfs {
    id: u64,
    subcadena_ruta: String,
    modo: ModoFalloVfs,
    checkpoint_truncado: Arc<AtomicBool>,
    disparos: Arc<AtomicUsize>,
}

#[repr(C)]
#[derive(Copy, Clone)]
struct ArchivoVfsExtra {
    orig_methods: *const ffi::sqlite3_io_methods,
    id_regla: u64,
    flags: c_int,
}

struct EstadoVfs {
    _base_vfs: usize,
    base_sz_alineado: usize,
    _vfs_box: usize,
}

static ESTADO_VFS: OnceLock<EstadoVfs> = OnceLock::new();
static REGLAS_VFS: Mutex<Vec<ReglaVfs>> = Mutex::new(Vec::new());
static SIGUIENTE_REGLA_VFS: AtomicU64 = AtomicU64::new(1);

const fn align_up(valor: usize, align: usize) -> usize {
    (valor + align - 1) & !(align - 1)
}

unsafe fn base_vfs_desde(p_vfs: *mut ffi::sqlite3_vfs) -> *mut ffi::sqlite3_vfs {
    unsafe { (*p_vfs).pAppData.cast::<ffi::sqlite3_vfs>() }
}

unsafe fn extra_archivo(p_file: *mut ffi::sqlite3_file) -> *mut ArchivoVfsExtra {
    let offset = ESTADO_VFS
        .get()
        .expect("VFS de pruebas inicializado")
        .base_sz_alineado;
    unsafe { p_file.cast::<u8>().add(offset).cast::<ArchivoVfsExtra>() }
}

unsafe fn orig_methods_de(p_file: *mut ffi::sqlite3_file) -> *const ffi::sqlite3_io_methods {
    unsafe { (*extra_archivo(p_file)).orig_methods }
}

fn buscar_regla_por_ruta(ruta: &str) -> Option<ReglaVfs> {
    REGLAS_VFS
        .lock()
        .expect("mutex de reglas VFS")
        .iter()
        .find(|r| ruta.contains(&r.subcadena_ruta))
        .cloned()
}

fn buscar_regla_por_id(id: u64) -> Option<ReglaVfs> {
    REGLAS_VFS
        .lock()
        .expect("mutex de reglas VFS")
        .iter()
        .find(|r| r.id == id)
        .cloned()
}

unsafe extern "C" fn vfs_open(
    p_vfs: *mut ffi::sqlite3_vfs,
    z_name: *const c_char,
    p_file: *mut ffi::sqlite3_file,
    flags: c_int,
    p_out_flags: *mut c_int,
) -> c_int {
    let base = unsafe { base_vfs_desde(p_vfs) };
    let rc =
        unsafe { ((*base).xOpen.expect("base xOpen"))(base, z_name, p_file, flags, p_out_flags) };
    if rc != ffi::SQLITE_OK || z_name.is_null() {
        return rc;
    }
    let ruta = unsafe { CStr::from_ptr(z_name) }.to_string_lossy();
    let Some(regla) = buscar_regla_por_ruta(&ruta) else {
        return rc;
    };
    let orig_methods = unsafe { (*p_file).pMethods };
    if orig_methods.is_null() {
        return rc;
    }
    unsafe {
        let extra = extra_archivo(p_file);
        extra.write(ArchivoVfsExtra {
            orig_methods,
            id_regla: regla.id,
            flags,
        });
        (*p_file).pMethods = if (*orig_methods).iVersion >= 3 && (*orig_methods).xFetch.is_some() {
            &raw const WRAPPER_IO_V3
        } else {
            &raw const WRAPPER_IO_V2
        };
    }
    rc
}

unsafe extern "C" fn vfs_delete(
    p_vfs: *mut ffi::sqlite3_vfs,
    z_name: *const c_char,
    sync_dir: c_int,
) -> c_int {
    let base = unsafe { base_vfs_desde(p_vfs) };
    unsafe { ((*base).xDelete.expect("base xDelete"))(base, z_name, sync_dir) }
}

unsafe extern "C" fn vfs_access(
    p_vfs: *mut ffi::sqlite3_vfs,
    z_name: *const c_char,
    flags: c_int,
    p_res_out: *mut c_int,
) -> c_int {
    let base = unsafe { base_vfs_desde(p_vfs) };
    unsafe { ((*base).xAccess.expect("base xAccess"))(base, z_name, flags, p_res_out) }
}

unsafe extern "C" fn vfs_full_pathname(
    p_vfs: *mut ffi::sqlite3_vfs,
    z_name: *const c_char,
    n_out: c_int,
    z_out: *mut c_char,
) -> c_int {
    let base = unsafe { base_vfs_desde(p_vfs) };
    unsafe { ((*base).xFullPathname.expect("base xFullPathname"))(base, z_name, n_out, z_out) }
}

unsafe extern "C" fn vfs_dl_open(
    p_vfs: *mut ffi::sqlite3_vfs,
    z_filename: *const c_char,
) -> *mut c_void {
    let base = unsafe { base_vfs_desde(p_vfs) };
    unsafe { ((*base).xDlOpen.expect("base xDlOpen"))(base, z_filename) }
}

unsafe extern "C" fn vfs_dl_error(
    p_vfs: *mut ffi::sqlite3_vfs,
    n_byte: c_int,
    z_err_msg: *mut c_char,
) {
    let base = unsafe { base_vfs_desde(p_vfs) };
    unsafe { ((*base).xDlError.expect("base xDlError"))(base, n_byte, z_err_msg) }
}

unsafe extern "C" fn vfs_dl_sym(
    p_vfs: *mut ffi::sqlite3_vfs,
    p_handle: *mut c_void,
    z_symbol: *const c_char,
) -> Option<unsafe extern "C" fn(*mut ffi::sqlite3_vfs, *mut c_void, *const c_char)> {
    let base = unsafe { base_vfs_desde(p_vfs) };
    unsafe { ((*base).xDlSym.expect("base xDlSym"))(base, p_handle, z_symbol) }
}

unsafe extern "C" fn vfs_dl_close(p_vfs: *mut ffi::sqlite3_vfs, p_handle: *mut c_void) {
    let base = unsafe { base_vfs_desde(p_vfs) };
    unsafe { ((*base).xDlClose.expect("base xDlClose"))(base, p_handle) }
}

unsafe extern "C" fn vfs_randomness(
    p_vfs: *mut ffi::sqlite3_vfs,
    n_byte: c_int,
    z_out: *mut c_char,
) -> c_int {
    let base = unsafe { base_vfs_desde(p_vfs) };
    unsafe { ((*base).xRandomness.expect("base xRandomness"))(base, n_byte, z_out) }
}

unsafe extern "C" fn vfs_sleep(p_vfs: *mut ffi::sqlite3_vfs, microseconds: c_int) -> c_int {
    let base = unsafe { base_vfs_desde(p_vfs) };
    unsafe { ((*base).xSleep.expect("base xSleep"))(base, microseconds) }
}

unsafe extern "C" fn vfs_current_time(p_vfs: *mut ffi::sqlite3_vfs, pr_now: *mut f64) -> c_int {
    let base = unsafe { base_vfs_desde(p_vfs) };
    unsafe { ((*base).xCurrentTime.expect("base xCurrentTime"))(base, pr_now) }
}

unsafe extern "C" fn vfs_get_last_error(
    p_vfs: *mut ffi::sqlite3_vfs,
    n_buf: c_int,
    z_buf: *mut c_char,
) -> c_int {
    let base = unsafe { base_vfs_desde(p_vfs) };
    unsafe { ((*base).xGetLastError.expect("base xGetLastError"))(base, n_buf, z_buf) }
}

unsafe extern "C" fn vfs_current_time_int64(
    p_vfs: *mut ffi::sqlite3_vfs,
    pi_now: *mut ffi::sqlite3_int64,
) -> c_int {
    let base = unsafe { base_vfs_desde(p_vfs) };
    unsafe { ((*base).xCurrentTimeInt64.expect("base xCurrentTimeInt64"))(base, pi_now) }
}

unsafe extern "C" fn vfs_set_system_call(
    p_vfs: *mut ffi::sqlite3_vfs,
    z_name: *const c_char,
    p_new_func: ffi::sqlite3_syscall_ptr,
) -> c_int {
    let base = unsafe { base_vfs_desde(p_vfs) };
    unsafe { ((*base).xSetSystemCall.expect("base xSetSystemCall"))(base, z_name, p_new_func) }
}

unsafe extern "C" fn vfs_get_system_call(
    p_vfs: *mut ffi::sqlite3_vfs,
    z_name: *const c_char,
) -> ffi::sqlite3_syscall_ptr {
    let base = unsafe { base_vfs_desde(p_vfs) };
    unsafe { ((*base).xGetSystemCall.expect("base xGetSystemCall"))(base, z_name) }
}

unsafe extern "C" fn vfs_next_system_call(
    p_vfs: *mut ffi::sqlite3_vfs,
    z_name: *const c_char,
) -> *const c_char {
    let base = unsafe { base_vfs_desde(p_vfs) };
    unsafe { ((*base).xNextSystemCall.expect("base xNextSystemCall"))(base, z_name) }
}

unsafe extern "C" fn io_close(p_file: *mut ffi::sqlite3_file) -> c_int {
    let orig = unsafe { orig_methods_de(p_file) };
    unsafe { ((*orig).xClose.expect("orig xClose"))(p_file) }
}

unsafe extern "C" fn io_read(
    p_file: *mut ffi::sqlite3_file,
    z_buf: *mut c_void,
    i_amt: c_int,
    i_ofst: ffi::sqlite3_int64,
) -> c_int {
    let orig = unsafe { orig_methods_de(p_file) };
    unsafe { ((*orig).xRead.expect("orig xRead"))(p_file, z_buf, i_amt, i_ofst) }
}

unsafe extern "C" fn io_write(
    p_file: *mut ffi::sqlite3_file,
    z_buf: *const c_void,
    i_amt: c_int,
    i_ofst: ffi::sqlite3_int64,
) -> c_int {
    let orig = unsafe { orig_methods_de(p_file) };
    unsafe { ((*orig).xWrite.expect("orig xWrite"))(p_file, z_buf, i_amt, i_ofst) }
}

unsafe extern "C" fn io_truncate(
    p_file: *mut ffi::sqlite3_file,
    size: ffi::sqlite3_int64,
) -> c_int {
    let extra = unsafe { &*extra_archivo(p_file) };
    if (extra.flags & ffi::SQLITE_OPEN_WAL) != 0
        && let Some(regla) = buscar_regla_por_id(extra.id_regla)
    {
        if regla.modo == ModoFalloVfs::FalloTruncateWalEnCheckpoint {
            regla.disparos.fetch_add(1, Ordering::SeqCst);
            return ffi::SQLITE_IOERR_TRUNCATE;
        }
        let orig = extra.orig_methods;
        let rc = unsafe { ((*orig).xTruncate.expect("orig xTruncate"))(p_file, size) };
        if rc == ffi::SQLITE_OK && size == 0 {
            regla.checkpoint_truncado.store(true, Ordering::SeqCst);
        }
        return rc;
    }
    let orig = extra.orig_methods;
    unsafe { ((*orig).xTruncate.expect("orig xTruncate"))(p_file, size) }
}

unsafe extern "C" fn io_sync(p_file: *mut ffi::sqlite3_file, flags: c_int) -> c_int {
    let orig = unsafe { orig_methods_de(p_file) };
    unsafe { ((*orig).xSync.expect("orig xSync"))(p_file, flags) }
}

unsafe extern "C" fn io_file_size(
    p_file: *mut ffi::sqlite3_file,
    p_size: *mut ffi::sqlite3_int64,
) -> c_int {
    let orig = unsafe { orig_methods_de(p_file) };
    unsafe { ((*orig).xFileSize.expect("orig xFileSize"))(p_file, p_size) }
}

unsafe extern "C" fn io_lock(p_file: *mut ffi::sqlite3_file, lock: c_int) -> c_int {
    let extra = unsafe { &*extra_archivo(p_file) };
    if (extra.flags & ffi::SQLITE_OPEN_MAIN_DB) != 0
        && lock == ffi::SQLITE_LOCK_EXCLUSIVE
        && let Some(regla) = buscar_regla_por_id(extra.id_regla)
        && regla.modo == ModoFalloVfs::FalloExclusiveLockEnTransicionDelete
        && regla.checkpoint_truncado.load(Ordering::SeqCst)
    {
        regla.disparos.fetch_add(1, Ordering::SeqCst);
        return ffi::SQLITE_IOERR_LOCK;
    }
    let orig = extra.orig_methods;
    unsafe { ((*orig).xLock.expect("orig xLock"))(p_file, lock) }
}

unsafe extern "C" fn io_unlock(p_file: *mut ffi::sqlite3_file, lock: c_int) -> c_int {
    let orig = unsafe { orig_methods_de(p_file) };
    unsafe { ((*orig).xUnlock.expect("orig xUnlock"))(p_file, lock) }
}

unsafe extern "C" fn io_check_reserved_lock(
    p_file: *mut ffi::sqlite3_file,
    p_res_out: *mut c_int,
) -> c_int {
    let orig = unsafe { orig_methods_de(p_file) };
    unsafe { ((*orig).xCheckReservedLock.expect("orig xCheckReservedLock"))(p_file, p_res_out) }
}

unsafe extern "C" fn io_file_control(
    p_file: *mut ffi::sqlite3_file,
    op: c_int,
    p_arg: *mut c_void,
) -> c_int {
    let orig = unsafe { orig_methods_de(p_file) };
    unsafe { ((*orig).xFileControl.expect("orig xFileControl"))(p_file, op, p_arg) }
}

unsafe extern "C" fn io_sector_size(p_file: *mut ffi::sqlite3_file) -> c_int {
    let orig = unsafe { orig_methods_de(p_file) };
    unsafe { ((*orig).xSectorSize.expect("orig xSectorSize"))(p_file) }
}

unsafe extern "C" fn io_device_characteristics(p_file: *mut ffi::sqlite3_file) -> c_int {
    let orig = unsafe { orig_methods_de(p_file) };
    unsafe {
        ((*orig)
            .xDeviceCharacteristics
            .expect("orig xDeviceCharacteristics"))(p_file)
    }
}

unsafe extern "C" fn io_shm_map(
    p_file: *mut ffi::sqlite3_file,
    i_pg: c_int,
    pgsz: c_int,
    is_write: c_int,
    pp: *mut *mut c_void,
) -> c_int {
    let orig = unsafe { orig_methods_de(p_file) };
    unsafe { ((*orig).xShmMap.expect("orig xShmMap"))(p_file, i_pg, pgsz, is_write, pp) }
}

unsafe extern "C" fn io_shm_lock(
    p_file: *mut ffi::sqlite3_file,
    offset: c_int,
    n: c_int,
    flags: c_int,
) -> c_int {
    let orig = unsafe { orig_methods_de(p_file) };
    unsafe { ((*orig).xShmLock.expect("orig xShmLock"))(p_file, offset, n, flags) }
}

unsafe extern "C" fn io_shm_barrier(p_file: *mut ffi::sqlite3_file) {
    let orig = unsafe { orig_methods_de(p_file) };
    unsafe { ((*orig).xShmBarrier.expect("orig xShmBarrier"))(p_file) }
}

unsafe extern "C" fn io_shm_unmap(p_file: *mut ffi::sqlite3_file, delete_flag: c_int) -> c_int {
    let orig = unsafe { orig_methods_de(p_file) };
    unsafe { ((*orig).xShmUnmap.expect("orig xShmUnmap"))(p_file, delete_flag) }
}

unsafe extern "C" fn io_fetch(
    p_file: *mut ffi::sqlite3_file,
    i_ofst: ffi::sqlite3_int64,
    i_amt: c_int,
    pp: *mut *mut c_void,
) -> c_int {
    let orig = unsafe { orig_methods_de(p_file) };
    unsafe { ((*orig).xFetch.expect("orig xFetch"))(p_file, i_ofst, i_amt, pp) }
}

unsafe extern "C" fn io_unfetch(
    p_file: *mut ffi::sqlite3_file,
    i_ofst: ffi::sqlite3_int64,
    p: *mut c_void,
) -> c_int {
    let orig = unsafe { orig_methods_de(p_file) };
    unsafe { ((*orig).xUnfetch.expect("orig xUnfetch"))(p_file, i_ofst, p) }
}

static WRAPPER_IO_V2: ffi::sqlite3_io_methods = ffi::sqlite3_io_methods {
    iVersion: 2,
    xClose: Some(io_close),
    xRead: Some(io_read),
    xWrite: Some(io_write),
    xTruncate: Some(io_truncate),
    xSync: Some(io_sync),
    xFileSize: Some(io_file_size),
    xLock: Some(io_lock),
    xUnlock: Some(io_unlock),
    xCheckReservedLock: Some(io_check_reserved_lock),
    xFileControl: Some(io_file_control),
    xSectorSize: Some(io_sector_size),
    xDeviceCharacteristics: Some(io_device_characteristics),
    xShmMap: Some(io_shm_map),
    xShmLock: Some(io_shm_lock),
    xShmBarrier: Some(io_shm_barrier),
    xShmUnmap: Some(io_shm_unmap),
    xFetch: None,
    xUnfetch: None,
};

static WRAPPER_IO_V3: ffi::sqlite3_io_methods = ffi::sqlite3_io_methods {
    iVersion: 3,
    xClose: Some(io_close),
    xRead: Some(io_read),
    xWrite: Some(io_write),
    xTruncate: Some(io_truncate),
    xSync: Some(io_sync),
    xFileSize: Some(io_file_size),
    xLock: Some(io_lock),
    xUnlock: Some(io_unlock),
    xCheckReservedLock: Some(io_check_reserved_lock),
    xFileControl: Some(io_file_control),
    xSectorSize: Some(io_sector_size),
    xDeviceCharacteristics: Some(io_device_characteristics),
    xShmMap: Some(io_shm_map),
    xShmLock: Some(io_shm_lock),
    xShmBarrier: Some(io_shm_barrier),
    xShmUnmap: Some(io_shm_unmap),
    xFetch: Some(io_fetch),
    xUnfetch: Some(io_unfetch),
};

fn asegurar_vfs_registrado() {
    let _ = ESTADO_VFS.get_or_init(|| unsafe {
        let base = ffi::sqlite3_vfs_find(ptr::null());
        assert!(!base.is_null(), "debe existir un VFS SQLite por defecto");
        let base_sz = usize::try_from((*base).szOsFile).expect("szOsFile positivo");
        let base_sz_alineado = align_up(base_sz, align_of::<ArchivoVfsExtra>());
        let sz_total = c_int::try_from(base_sz_alineado + size_of::<ArchivoVfsExtra>())
            .expect("szOsFile total cabe en c_int");

        let vfs = Box::new(ffi::sqlite3_vfs {
            iVersion: (*base).iVersion.min(3),
            szOsFile: sz_total,
            mxPathname: (*base).mxPathname,
            pNext: ptr::null_mut(),
            zName: c"sgt_test_vfs".as_ptr(),
            pAppData: base.cast::<c_void>(),
            xOpen: Some(vfs_open),
            xDelete: Some(vfs_delete),
            xAccess: Some(vfs_access),
            xFullPathname: Some(vfs_full_pathname),
            xDlOpen: (*base).xDlOpen.map(|_| vfs_dl_open as _),
            xDlError: (*base).xDlError.map(|_| vfs_dl_error as _),
            xDlSym: (*base).xDlSym.map(|_| vfs_dl_sym as _),
            xDlClose: (*base).xDlClose.map(|_| vfs_dl_close as _),
            xRandomness: (*base).xRandomness.map(|_| vfs_randomness as _),
            xSleep: (*base).xSleep.map(|_| vfs_sleep as _),
            xCurrentTime: (*base).xCurrentTime.map(|_| vfs_current_time as _),
            xGetLastError: (*base).xGetLastError.map(|_| vfs_get_last_error as _),
            xCurrentTimeInt64: (*base)
                .xCurrentTimeInt64
                .map(|_| vfs_current_time_int64 as _),
            xSetSystemCall: (*base).xSetSystemCall.map(|_| vfs_set_system_call as _),
            xGetSystemCall: (*base).xGetSystemCall.map(|_| vfs_get_system_call as _),
            xNextSystemCall: (*base).xNextSystemCall.map(|_| vfs_next_system_call as _),
        });
        let vfs_ptr = Box::into_raw(vfs);
        let rc = ffi::sqlite3_vfs_register(vfs_ptr, 1);
        assert_eq!(rc, ffi::SQLITE_OK, "debe registrar VFS wrapper");
        EstadoVfs {
            _base_vfs: base as usize,
            base_sz_alineado,
            _vfs_box: vfs_ptr as usize,
        }
    });
}

struct GuardiaReglaVfs {
    id: u64,
}

impl GuardiaReglaVfs {
    fn instalar(dir: &DirectorioPrueba, modo: ModoFalloVfs) -> (Self, Arc<AtomicUsize>) {
        asegurar_vfs_registrado();
        let id = SIGUIENTE_REGLA_VFS.fetch_add(1, Ordering::Relaxed);
        let subcadena_ruta = dir
            .raiz
            .file_name()
            .expect("directorio de prueba debe tener nombre")
            .to_string_lossy()
            .into_owned();
        let disparos = Arc::new(AtomicUsize::new(0));
        REGLAS_VFS
            .lock()
            .expect("mutex de reglas VFS")
            .push(ReglaVfs {
                id,
                subcadena_ruta,
                modo,
                checkpoint_truncado: Arc::new(AtomicBool::new(false)),
                disparos: Arc::clone(&disparos),
            });
        (Self { id }, disparos)
    }
}

impl Drop for GuardiaReglaVfs {
    fn drop(&mut self) {
        if let Ok(mut reglas) = REGLAS_VFS.lock() {
            reglas.retain(|r| r.id != self.id);
        }
    }
}

#[test]
fn fallo_real_de_checkpoint_y_transicion_wal_via_vfs_devuelve_error_persistencia_sin_publicar_db() {
    // 1. Fallo real durante `PRAGMA wal_checkpoint(TRUNCATE)` (`xTruncate` sobre `-wal`).
    {
        let dir = DirectorioPrueba::nuevo("vfs_fallo_checkpoint");
        let (guardia, disparos) =
            GuardiaReglaVfs::instalar(&dir, ModoFalloVfs::FalloTruncateWalEnCheckpoint);

        let err = RepositorioSqlite::abrir(dir.ruta())
            .err()
            .expect("fallo real de xTruncate en checkpoint debe abortar apertura nueva");
        assert_eq!(err.codigo, CodigoError::ErrorPersistencia);
        assert!(
            disparos.load(Ordering::SeqCst) >= 1,
            "el VFS debi? interceptar xTruncate durante wal_checkpoint(TRUNCATE)"
        );
        assert!(
            !dir.ruta().exists(),
            "no debe publicarse sgt.db cuando falla el checkpoint temporal"
        );
        let restantes: Vec<_> = fs::read_dir(&dir.raiz)
            .expect("debe listar directorio")
            .filter_map(Result::ok)
            .collect();
        assert!(
            restantes.is_empty(),
            "la limpieza de BaseTemporal no debe dejar residuos tras fallo de checkpoint: {restantes:?}"
        );

        drop(guardia);

        // Sin el fallo VFS, abrir en la misma ruta publica e inicializa correctamente.
        let mut repo =
            RepositorioSqlite::abrir(dir.ruta()).expect("tras retirar fallo VFS debe abrir");
        assert_eq!(
            repo.cargar().expect("debe cargar semillas"),
            SistemaTurnos::nuevo()
        );
    }

    // 2. Fallo real durante `PRAGMA journal_mode = DELETE` (`xLock(EXCLUSIVE)` en `sqlite3WalClose`).
    {
        let dir = DirectorioPrueba::nuevo("vfs_fallo_transicion_delete");
        let (guardia, disparos) =
            GuardiaReglaVfs::instalar(&dir, ModoFalloVfs::FalloExclusiveLockEnTransicionDelete);

        let err = RepositorioSqlite::abrir(dir.ruta())
            .err()
            .expect("fallo real de transici?n WAL->DELETE debe abortar apertura nueva");
        assert_eq!(err.codigo, CodigoError::ErrorPersistencia);
        assert!(
            disparos.load(Ordering::SeqCst) >= 1,
            "el VFS debi? interceptar xLock(EXCLUSIVE) durante PRAGMA journal_mode = DELETE"
        );
        assert!(
            !dir.ruta().exists(),
            "no debe publicarse sgt.db cuando falla la transici?n WAL->DELETE"
        );
        let restantes: Vec<_> = fs::read_dir(&dir.raiz)
            .expect("debe listar directorio")
            .filter_map(Result::ok)
            .collect();
        assert!(
            restantes.is_empty(),
            "la limpieza de BaseTemporal no debe dejar residuos tras fallo WAL->DELETE: {restantes:?}"
        );

        drop(guardia);

        let mut repo =
            RepositorioSqlite::abrir(dir.ruta()).expect("tras retirar fallo VFS debe abrir");
        assert_eq!(
            repo.cargar().expect("debe cargar semillas"),
            SistemaTurnos::nuevo()
        );
    }
}
