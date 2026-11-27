//! Inicialización y configuración de la base de datos SQLite del SGT.
//!
//! Este módulo centraliza la apertura de conexiones SQLite, la activación de
//! `PRAGMA`s de integridad (`foreign_keys`, `WAL`, `busy_timeout`) y la
//! aplicación idempotente del esquema inicial definido en
//! `src-tauri/migrations/001_initial.sql`.

use std::fs;
use std::path::Path;

use rusqlite::Connection;

/// Script SQL de la migración inicial (`001_initial.sql`).
pub const INITIAL_SCHEMA_SQL: &str = include_str!("../migrations/001_initial.sql");

/// Nombre por defecto del archivo de base de datos dentro de `app_data_dir`.
pub const DEFAULT_DB_FILENAME: &str = "sgt.db";

/// Configura los `PRAGMA`s obligatorios de sesión en una conexión SQLite.
///
/// Debe ejecutarse fuera de cualquier transacción activa para garantizar que
/// `foreign_keys = ON` y `journal_mode = WAL` surtan efecto.
pub fn configurar_pragmas(conn: &Connection) -> Result<(), rusqlite::Error> {
    conn.execute_batch(
        "PRAGMA foreign_keys = ON;
         PRAGMA journal_mode = WAL;
         PRAGMA synchronous = NORMAL;
         PRAGMA busy_timeout = 5000;",
    )
}

/// Aplica de forma idempotente el esquema inicial SQLite sobre una conexión.
pub fn inicializar_esquema(conn: &Connection) -> Result<(), rusqlite::Error> {
    configurar_pragmas(conn)?;
    conn.execute_batch(INITIAL_SCHEMA_SQL)
}

/// Abre (o crea) la base de datos SQLite en `ruta`, creando directorios padres
/// si aún no existen, y asegura que el esquema inicial esté aplicado.
pub fn abrir_conexion(ruta: impl AsRef<Path>) -> Result<Connection, rusqlite::Error> {
    let conn = abrir_conexion_configurada(ruta)?;
    inicializar_esquema(&conn)?;
    Ok(conn)
}

/// Abre y configura una conexión sin aplicar el esquema ni insertar semillas.
///
/// Permite validar una base existente antes de cualquier inicialización.
///
/// # Errors
/// Devuelve el error de creación del directorio, apertura o configuración.
pub fn abrir_conexion_configurada(ruta: impl AsRef<Path>) -> Result<Connection, rusqlite::Error> {
    let ruta = ruta.as_ref();
    if let Some(padre) = ruta.parent()
        && !padre.as_os_str().is_empty()
        && !padre.exists()
    {
        fs::create_dir_all(padre).map_err(|err| {
            rusqlite::Error::ToSqlConversionFailure(Box::new(std::io::Error::new(
                err.kind(),
                format!(
                    "no se pudo crear el directorio de base de datos '{}': {err}",
                    padre.display()
                ),
            )))
        })?;
    }

    let conn = Connection::open(ruta)?;
    configurar_pragmas(&conn)?;
    Ok(conn)
}

/// Abre una base de datos SQLite en memoria con el esquema inicial aplicado.
///
/// Útil para pruebas unitarias aisladas y validación rápida del esquema.
pub fn abrir_en_memoria() -> Result<Connection, rusqlite::Error> {
    let conn = Connection::open_in_memory()?;
    inicializar_esquema(&conn)?;
    Ok(conn)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn ruta_temporal_unica(etiqueta: &str) -> std::path::PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("el reloj del sistema debe ser posterior a UNIX_EPOCH")
            .as_nanos();
        std::env::temp_dir()
            .join(format!(
                "sgt_test_{etiqueta}_{}_{nanos}",
                std::process::id()
            ))
            .join(DEFAULT_DB_FILENAME)
    }

    #[test]
    fn inicializa_tablas_indices_triggers_y_semillas_de_forma_idempotente() {
        let conn = abrir_en_memoria().expect("debe abrir e inicializar en memoria");
        // Re-ejecutar el esquema no debe fallar ni duplicar registros.
        inicializar_esquema(&conn).expect("la inicialización debe ser idempotente");

        let fk_activo: i64 = conn
            .query_row("PRAGMA foreign_keys;", [], |r| r.get(0))
            .expect("debe consultar PRAGMA foreign_keys");
        assert_eq!(fk_activo, 1);

        for tabla in ["schema_migrations", "secuencias", "ventanillas", "turnos"] {
            let existe: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1;",
                    [tabla],
                    |r| r.get(0),
                )
                .expect("debe consultar sqlite_master");
            assert_eq!(existe, 1, "falta la tabla {tabla}");
        }

        for indice in [
            "ux_turnos_un_activo_por_ventanilla",
            "ix_turnos_cola_fifo",
            "ix_turnos_estado_secuencia",
        ] {
            let existe: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND name = ?1;",
                    [indice],
                    |r| r.get(0),
                )
                .expect("debe consultar índices");
            assert_eq!(existe, 1, "falta el índice {indice}");
        }

        let secuencia_inicial: i64 = conn
            .query_row(
                "SELECT ultimo_valor FROM secuencias WHERE clave = 'turno';",
                [],
                |r| r.get(0),
            )
            .expect("debe existir la secuencia inicial de turnos");
        assert_eq!(secuencia_inicial, 0);

        let mut stmt = conn
            .prepare("SELECT id, habilitada, turno_actual_id FROM ventanillas ORDER BY id ASC;")
            .expect("debe preparar consulta de ventanillas");
        let ventanillas: Vec<(String, i64, Option<String>)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .expect("debe mapear ventanillas")
            .collect::<Result<_, _>>()
            .expect("debe recolectar ventanillas");

        assert_eq!(
            ventanillas,
            vec![
                ("V01".to_string(), 1, None),
                ("V02".to_string(), 1, None),
                ("V03".to_string(), 1, None),
            ]
        );
    }

    #[test]
    fn inicializa_en_archivo_real_y_conserva_datos_al_reabrir() {
        let ruta = ruta_temporal_unica("persistencia");
        let dir_prueba = ruta
            .parent()
            .expect("ruta temporal debe tener padre")
            .to_path_buf();

        {
            let conn = abrir_conexion(&ruta).expect("debe crear BD en disco");
            conn.execute(
                "UPDATE secuencias SET ultimo_valor = 1 WHERE clave = 'turno';",
                [],
            )
            .expect("debe incrementar secuencia");
            conn.execute(
                "INSERT INTO turnos (id, secuencia, fecha_hora_registro, estado, ventanilla_asignada)
                 VALUES ('T-0001', 1, '2026-11-23T20:00:00Z', 'EN_ATENCION', 'V02');",
                [],
            )
            .expect("debe insertar turno en atención");
            conn.execute(
                "UPDATE ventanillas SET turno_actual_id = 'T-0001' WHERE id = 'V02';",
                [],
            )
            .expect("debe asociar turno a V02");
        }

        {
            let conn = abrir_conexion(&ruta).expect("debe reabrir BD existente sin sobrescribir");
            let ultimo_valor: i64 = conn
                .query_row(
                    "SELECT ultimo_valor FROM secuencias WHERE clave = 'turno';",
                    [],
                    |r| r.get(0),
                )
                .expect("debe leer secuencia");
            let turno_v02: Option<String> = conn
                .query_row(
                    "SELECT turno_actual_id FROM ventanillas WHERE id = 'V02';",
                    [],
                    |r| r.get(0),
                )
                .expect("debe leer V02");

            assert_eq!(ultimo_valor, 1);
            assert_eq!(turno_v02.as_deref(), Some("T-0001"));
        }

        let _ = fs::remove_dir_all(&dir_prueba);
    }

    #[test]
    fn valida_restricciones_de_turnos_estados_y_ventanillas() {
        let conn = abrir_en_memoria().expect("debe abrir BD en memoria");

        // Estado inválido rechazado por CHECK.
        assert!(
            conn.execute(
                "INSERT INTO turnos (id, secuencia, fecha_hora_registro, estado, ventanilla_asignada)
                 VALUES ('T-0001', 1, '2026-11-23T20:00:00Z', 'PAUSADO', NULL);",
                [],
            )
            .is_err()
        );

        // Secuencia <= 0 rechazada por CHECK.
        assert!(
            conn.execute(
                "INSERT INTO turnos (id, secuencia, fecha_hora_registro, estado, ventanilla_asignada)
                 VALUES ('T-0001', 0, '2026-11-23T20:00:00Z', 'EN_ESPERA', NULL);",
                [],
            )
            .is_err()
        );

        // EN_ESPERA con ventanilla asignada rechazado por CHECK de coherencia.
        assert!(
            conn.execute(
                "INSERT INTO turnos (id, secuencia, fecha_hora_registro, estado, ventanilla_asignada)
                 VALUES ('T-0001', 1, '2026-11-23T20:00:00Z', 'EN_ESPERA', 'V01');",
                [],
            )
            .is_err()
        );

        // EN_ATENCION sin ventanilla rechazado por CHECK de coherencia.
        assert!(
            conn.execute(
                "INSERT INTO turnos (id, secuencia, fecha_hora_registro, estado, ventanilla_asignada)
                 VALUES ('T-0001', 1, '2026-11-23T20:00:00Z', 'EN_ATENCION', NULL);",
                [],
            )
            .is_err()
        );

        // Ventanilla inexistente ('V04' no insertada) rechazada por FOREIGN KEY.
        assert!(
            conn.execute(
                "INSERT INTO turnos (id, secuencia, fecha_hora_registro, estado, ventanilla_asignada)
                 VALUES ('T-0001', 1, '2026-11-23T20:00:00Z', 'EN_ATENCION', 'V04');",
                [],
            )
            .is_err()
        );

        // ID de ventanilla con formato inválido rechazado por CHECK.
        assert!(
            conn.execute(
                "INSERT INTO ventanillas (id, habilitada, turno_actual_id) VALUES ('CAJA1', 1, NULL);",
                [],
            )
            .is_err()
        );
    }

    #[test]
    fn impide_duplicar_atencion_por_ventanilla_o_asignar_mismo_turno_dos_veces() {
        let conn = abrir_en_memoria().expect("debe abrir BD en memoria");

        conn.execute(
            "INSERT INTO turnos (id, secuencia, fecha_hora_registro, estado, ventanilla_asignada)
             VALUES ('T-0001', 1, '2026-11-23T20:00:00Z', 'EN_ATENCION', 'V01');",
            [],
        )
        .expect("primer turno en atención en V01 debe insertarse");

        conn.execute(
            "UPDATE ventanillas SET turno_actual_id = 'T-0001' WHERE id = 'V01';",
            [],
        )
        .expect("V01 debe asociarse a T-0001");

        // RN-04: segundo turno EN_ATENCION en la misma ventanilla V01 debe fallar.
        assert!(
            conn.execute(
                "INSERT INTO turnos (id, secuencia, fecha_hora_registro, estado, ventanilla_asignada)
                 VALUES ('T-0002', 2, '2026-11-23T20:01:00Z', 'EN_ATENCION', 'V01');",
                [],
            )
            .is_err()
        );

        // RN-05: mismo turno T-0001 en otra ventanilla V02 debe fallar por UNIQUE.
        assert!(
            conn.execute(
                "UPDATE ventanillas SET turno_actual_id = 'T-0001' WHERE id = 'V02';",
                [],
            )
            .is_err()
        );

        // RF-13: no se puede inhabilitar una ventanilla mientras tiene un turno asignado.
        assert!(
            conn.execute(
                "UPDATE ventanillas SET habilitada = 0 WHERE id = 'V01';",
                []
            )
            .is_err()
        );
    }

    #[test]
    fn protege_regla_minimo_una_ventanilla_activa_inmutabilidad_y_extensibilidad_v04() {
        let conn = abrir_en_memoria().expect("debe abrir BD en memoria");

        // RF-13: se pueden inhabilitar 2 cajas libres, pero nunca la tercera.
        conn.execute(
            "UPDATE ventanillas SET habilitada = 0 WHERE id = 'V01';",
            [],
        )
        .expect("inhabilitar V01 libre debe permitirse");
        conn.execute(
            "UPDATE ventanillas SET habilitada = 0 WHERE id = 'V02';",
            [],
        )
        .expect("inhabilitar V02 libre debe permitirse");
        assert!(
            conn.execute(
                "UPDATE ventanillas SET habilitada = 0 WHERE id = 'V03';",
                []
            )
            .is_err(),
            "no se debe permitir inhabilitar la última ventanilla activa"
        );

        // RC-03: no se pueden eliminar ventanillas.
        assert!(
            conn.execute("DELETE FROM ventanillas WHERE id = 'V01';", [])
                .is_err()
        );

        // Extensibilidad: agregar una futura V04 en ventanillas funciona sin tocar turnos.
        conn.execute(
            "INSERT INTO ventanillas (id, habilitada, turno_actual_id) VALUES ('V04', 1, NULL);",
            [],
        )
        .expect("agregar una cuarta ventanilla en el catálogo debe ser directo");
        conn.execute(
            "INSERT INTO turnos (id, secuencia, fecha_hora_registro, estado, ventanilla_asignada)
             VALUES ('T-0001', 1, '2026-11-23T20:00:00Z', 'ATENDIDO', 'V04');",
            [],
        )
        .expect("turnos debe aceptar cualquier ventanilla registrada en el catálogo");

        // RN-09: no se pueden eliminar ni mutar turnos en estado terminal.
        assert!(
            conn.execute("DELETE FROM turnos WHERE id = 'T-0001';", [])
                .is_err()
        );
        assert!(
            conn.execute(
                "UPDATE turnos SET estado = 'EN_ESPERA', ventanilla_asignada = NULL WHERE id = 'T-0001';",
                [],
            )
            .is_err()
        );
    }
}
