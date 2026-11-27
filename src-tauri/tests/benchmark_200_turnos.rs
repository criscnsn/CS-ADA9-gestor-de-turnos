//! Benchmark e integración de rendimiento con 200 turnos en SQLite (`#07-B` / `RNF-P01`).
//!
//! Verifica sobre una base de datos SQLite temporal real en disco (sin tocar
//! `app_data_dir` del usuario) precargada con **200 turnos** que el servicio
//! real de producción ([`Sgt`] + [`RepositorioSqlite`] vía [`iniciar_sgt_en_ruta`])
//! ejecuta en **≤ 2 segundos (2 000 ms)** cada una de las operaciones críticas
//! exigidas por `RNF-P01`:
//! 1. Restauración inicial desde SQLite (`iniciar_sgt_en_ruta` / `Repositorio::cargar`),
//! 2. Consulta de estado y fila de espera FIFO (`Sgt::obtener_estado` — `RF-03`, `RN-02`),
//! 3. Registro de nuevo turno (`SistemaTurnos::registrar_turno` — `RF-02`, `RN-02`),
//! 4. Asignación del siguiente turno a ventanilla (`SistemaTurnos::asignar_siguiente` — `RF-04`),
//! 5. Finalización de atención en ventanilla (`SistemaTurnos::finalizar_atencion` — `RF-06`).

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use chrono::{TimeZone, Utc};
use gestor_de_turnos_lib::comandos::{Sgt, iniciar_sgt_en_ruta};
use gestor_de_turnos_lib::db::{DEFAULT_DB_FILENAME, abrir_conexion};
use gestor_de_turnos_lib::domain::{SistemaTurnos, formatear_fecha_iso8601, formatear_id_turno};
use gestor_de_turnos_lib::models::{EstadoSistema, EstadoTurno, VentanillaId};
use rusqlite::{TransactionBehavior, params};

/// Umbral máximo permitido por `RNF-P01` (2 segundos por operación).
const UMBRAL_RNF_P01: Duration = Duration::from_secs(2);

/// Volumen base mínimo exigido en SQLite antes de iniciar las mediciones (`RNF-P01`).
const VOLUMEN_BASE_TURNOS: u64 = 200;

/// Iteraciones adicionales de medición estadística tras validar el dataset inicial.
const ITERACIONES_SERIE: usize = 25;

/// Guardián RAII que elimina el directorio temporal de prueba al salir de ámbito,
/// garantizando que nunca se ensucie el disco ni se altere la BD del usuario.
struct BaseDatosTemporal {
    directorio: PathBuf,
    ruta_archivo: PathBuf,
}

impl BaseDatosTemporal {
    fn nueva(etiqueta: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("el reloj del sistema debe ser posterior a UNIX_EPOCH")
            .as_nanos();
        let directorio = std::env::temp_dir().join(format!(
            "sgt_bench_7b_{etiqueta}_{}_{nanos}",
            std::process::id()
        ));
        let ruta_archivo = directorio.join(DEFAULT_DB_FILENAME);
        Self {
            directorio,
            ruta_archivo,
        }
    }

    fn ruta(&self) -> &Path {
        &self.ruta_archivo
    }
}

impl Drop for BaseDatosTemporal {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.directorio);
    }
}

/// Estadísticas resumidas en microsegundos para una serie de mediciones.
#[derive(Debug, Clone)]
struct EstadisticasOperacion {
    operacion: &'static str,
    muestras: usize,
    primera_us: u128,
    min_us: u128,
    promedio_us: u128,
    p95_us: u128,
    max_us: u128,
}

impl EstadisticasOperacion {
    fn calcular(operacion: &'static str, duraciones: &[Duration]) -> Self {
        assert!(
            !duraciones.is_empty(),
            "se requiere al menos una muestra para {operacion}"
        );
        let primera_us = duraciones[0].as_micros();
        let mut ordenadas: Vec<u128> = duraciones.iter().map(Duration::as_micros).collect();
        ordenadas.sort_unstable();

        let muestras = ordenadas.len();
        let min_us = ordenadas[0];
        let max_us = ordenadas[muestras - 1];
        let suma: u128 = ordenadas.iter().copied().sum();
        let promedio_us = suma / (muestras as u128);
        let indice_p95 = ((muestras * 95).div_ceil(100)).saturating_sub(1);
        let p95_us = ordenadas[indice_p95];

        Self {
            operacion,
            muestras,
            primera_us,
            min_us,
            promedio_us,
            p95_us,
            max_us,
        }
    }
}

/// Precarga en SQLite un dataset determinista y realista de exactamente 200 turnos:
/// - `1..=100` (100 turnos): `ATENDIDO` repartidos en `V01`, `V02` y `V03`.
/// - `101..=120` (20 turnos): `CANCELADO` (`ventanilla_asignada = NULL`).
/// - `121` (1 turno): `EN_ATENCION` en `V02` (`V01` y `V03` libres para medir asignación).
/// - `122..=200` (79 turnos): `EN_ESPERA` en la cola FIFO.
fn sembrar_dataset_200_turnos(ruta: &Path) {
    let mut conn = abrir_conexion(ruta).expect("debe crear e inicializar la BD temporal");
    let base_ts = Utc
        .with_ymd_and_hms(2026, 11, 25, 14, 0, 0)
        .single()
        .expect("fecha base válida");

    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .expect("debe abrir transacción de precarga");

    {
        let mut stmt_insert = tx
            .prepare(
                "INSERT INTO turnos (id, secuencia, fecha_hora_registro, estado, ventanilla_asignada)
                 VALUES (?1, ?2, ?3, ?4, ?5);",
            )
            .expect("debe preparar INSERT de precarga");

        for seq in 1..=VOLUMEN_BASE_TURNOS {
            let id = formatear_id_turno(seq);
            let instante = base_ts + chrono::Duration::seconds(i64::try_from(seq).unwrap() * 15);
            let fecha = formatear_fecha_iso8601(instante);

            let (estado, ventanilla): (&str, Option<&str>) = match seq {
                1..=100 => {
                    let caja = match seq % 3 {
                        1 => "V01",
                        2 => "V02",
                        _ => "V03",
                    };
                    ("ATENDIDO", Some(caja))
                }
                101..=120 => ("CANCELADO", None),
                121 => ("EN_ATENCION", Some("V02")),
                _ => ("EN_ESPERA", None),
            };

            stmt_insert
                .execute(params![
                    id,
                    i64::try_from(seq).unwrap(),
                    fecha,
                    estado,
                    ventanilla
                ])
                .expect("debe insertar fila de dataset base");
        }
    }

    tx.execute(
        "UPDATE ventanillas SET turno_actual_id = 'T-0121' WHERE id = 'V02';",
        [],
    )
    .expect("debe enlazar turno activo T-0121 a V02");

    tx.execute(
        "UPDATE secuencias SET ultimo_valor = ?1 WHERE clave = 'turno';",
        params![i64::try_from(VOLUMEN_BASE_TURNOS).unwrap()],
    )
    .expect("debe actualizar secuencia acumulada a 200");

    tx.commit()
        .expect("debe confirmar precarga de los 200 turnos en disco");
}

fn medir<T, F: FnOnce() -> T>(operacion: F) -> (T, Duration) {
    let inicio = Instant::now();
    let resultado = operacion();
    let duracion = inicio.elapsed();
    (resultado, duracion)
}

#[test]
fn rnf_p01_operaciones_con_200_turnos_en_sqlite_responden_en_menos_de_2_segundos() {
    let bd_temp = BaseDatosTemporal::nueva("rnf_p01");
    sembrar_dataset_200_turnos(bd_temp.ruta());

    // Reabrir desde disco con el coordinador real (`Sgt` + `RepositorioSqlite`)
    // validando `RD-03` e hidratando los 200 turnos antes de iniciar la serie.
    let (mut sgt, t_carga_inicial): (Sgt, Duration) = medir(|| {
        iniciar_sgt_en_ruta(bd_temp.ruta())
            .expect("debe abrir RepositorioSqlite y restaurar los 200 turnos")
    });
    assert!(
        t_carga_inicial <= UMBRAL_RNF_P01,
        "iniciar_sgt_en_ruta excedió 2s con 200 turnos: {t_carga_inicial:?}"
    );
    assert_eq!(
        u64::try_from(sgt.obtener_estado().historial.len()).unwrap(),
        VOLUMEN_BASE_TURNOS
    );

    let mut tiempos_consulta = Vec::with_capacity(ITERACIONES_SERIE);
    let mut tiempos_registro = Vec::with_capacity(ITERACIONES_SERIE);
    let mut tiempos_asignacion = Vec::with_capacity(ITERACIONES_SERIE);
    let mut tiempos_finalizacion = Vec::with_capacity(ITERACIONES_SERIE);
    let mut eventos_emitidos: usize = 0;

    for indice in 0..ITERACIONES_SERIE {
        let sec_esperada_registro = VOLUMEN_BASE_TURNOS + u64::try_from(indice).unwrap() + 1;
        let sec_esperada_fifo = 122 + u64::try_from(indice).unwrap();

        // 1. Consulta de estado y fila de espera FIFO (con >= 200 turnos).
        let (estado_actual, t_consulta) = medir(|| sgt.obtener_estado());
        let fila = &estado_actual.fila_espera;
        assert!(
            !fila.is_empty(),
            "debe haber turnos en espera durante la serie"
        );
        assert_eq!(fila[0].secuencia, sec_esperada_fifo);
        assert!(
            fila.windows(2)
                .all(|par| par[0].secuencia < par[1].secuencia),
            "la fila consultada debe respetar orden estrictamente creciente"
        );
        assert!(
            t_consulta <= UMBRAL_RNF_P01,
            "obtener_estado excedió 2s en iteración {indice}: {t_consulta:?}"
        );
        tiempos_consulta.push(t_consulta);

        // 2. Registro de turno nuevo con patrón real `commit SQLite -> memoria -> evento`.
        let mut ultimo_evento: Option<EstadoSistema> = None;
        let (registrado, t_registro) = medir(|| {
            sgt.ejecutar(SistemaTurnos::registrar_turno, |snapshot| {
                eventos_emitidos += 1;
                ultimo_evento = Some(snapshot);
            })
            .expect("registro de turno exitoso en RepositorioSqlite")
        });
        assert_eq!(registrado.secuencia, sec_esperada_registro);
        assert_eq!(registrado.id, formatear_id_turno(sec_esperada_registro));
        assert_eq!(registrado.estado, EstadoTurno::EnEspera);
        assert!(ultimo_evento.is_some(), "debe emitir snapshot tras commit");
        assert!(
            t_registro <= UMBRAL_RNF_P01,
            "registrar_turno excedió 2s en iteración {indice}: {t_registro:?}"
        );
        tiempos_registro.push(t_registro);

        // 3. Asignación del siguiente turno FIFO en V01 libre.
        let mut evento_asignacion: Option<EstadoSistema> = None;
        let (asignado, t_asignacion) = medir(|| {
            sgt.ejecutar(
                |s| s.asignar_siguiente(VentanillaId::V01),
                |snapshot| {
                    eventos_emitidos += 1;
                    evento_asignacion = Some(snapshot);
                },
            )
            .expect("asignación FIFO en V01 exitosa en RepositorioSqlite")
        });
        assert_eq!(asignado.secuencia, sec_esperada_fifo);
        assert_eq!(asignado.estado, EstadoTurno::EnAtencion);
        assert_eq!(asignado.ventanilla_asignada, Some(VentanillaId::V01));
        assert!(evento_asignacion.is_some());
        assert!(
            t_asignacion <= UMBRAL_RNF_P01,
            "asignar_siguiente excedió 2s en iteración {indice}: {t_asignacion:?}"
        );
        tiempos_asignacion.push(t_asignacion);

        // 4. Finalización de atención en V01 liberando la ventanilla.
        let mut evento_finalizacion: Option<EstadoSistema> = None;
        let (finalizado, t_finalizacion) = medir(|| {
            sgt.ejecutar(
                |s| s.finalizar_atencion(VentanillaId::V01),
                |snapshot| {
                    eventos_emitidos += 1;
                    evento_finalizacion = Some(snapshot);
                },
            )
            .expect("finalización en V01 exitosa en RepositorioSqlite")
        });
        assert_eq!(finalizado.id, asignado.id);
        assert_eq!(finalizado.estado, EstadoTurno::Atendido);
        assert_eq!(finalizado.ventanilla_asignada, Some(VentanillaId::V01));
        assert!(evento_finalizacion.is_some());
        assert!(
            t_finalizacion <= UMBRAL_RNF_P01,
            "finalizar_atencion excedió 2s en iteración {indice}: {t_finalizacion:?}"
        );
        tiempos_finalizacion.push(t_finalizacion);
    }

    assert_eq!(eventos_emitidos, ITERACIONES_SERIE * 3);
    let estado_final_memoria = sgt.obtener_estado();
    drop(sgt);

    // Reabrir desde disco para certificar que las 75 transacciones se persistieron
    // íntegramente y que la lectura completa en SQLite con 225 turnos cumple <= 2s.
    let (sgt_reabierto, t_reapertura) = medir(|| {
        iniciar_sgt_en_ruta(bd_temp.ruta())
            .expect("debe reabrir y restaurar los 225 turnos desde SQLite")
    });
    assert!(
        t_reapertura <= UMBRAL_RNF_P01,
        "reapertura desde SQLite con 225 turnos excedió 2s: {t_reapertura:?}"
    );
    let estado_reabierto = sgt_reabierto.obtener_estado();
    assert_eq!(estado_reabierto, estado_final_memoria);
    assert_eq!(
        u64::try_from(estado_reabierto.historial.len()).unwrap(),
        VOLUMEN_BASE_TURNOS + u64::try_from(ITERACIONES_SERIE).unwrap()
    );

    let tiempos_carga_disco = [t_carga_inicial, t_reapertura];
    let resumen = [
        EstadisticasOperacion::calcular("cargar_sqlite (RF-11)", &tiempos_carga_disco),
        EstadisticasOperacion::calcular("obtener_estado (RF-03)", &tiempos_consulta),
        EstadisticasOperacion::calcular("registrar_turno (RF-02)", &tiempos_registro),
        EstadisticasOperacion::calcular("asignar_siguiente (RF-04)", &tiempos_asignacion),
        EstadisticasOperacion::calcular("finalizar_atencion (RF-06)", &tiempos_finalizacion),
    ];

    println!("\n=== REPORTE BENCHMARK RNF-P01 (SGT + REPOSITORIOSQLITE REAL EN DISCO) ===");
    println!(
        "| Operación | Muestras | 1ra (µs) | Min (µs) | Promedio (µs) | P95 (µs) | Max (µs) | Max (ms) | Umbral |"
    );
    println!("| :--- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | :--- |");
    for item in &resumen {
        let max_ms = (item.max_us as f64) / 1000.0;
        println!(
            "| {} | {} | {} | {} | {} | {} | {} | {:.3} | <= 2000 ms |",
            item.operacion,
            item.muestras,
            item.primera_us,
            item.min_us,
            item.promedio_us,
            item.p95_us,
            item.max_us,
            max_ms
        );
        assert!(item.max_us <= UMBRAL_RNF_P01.as_micros());
    }
}

#[test]
fn dataset_precargado_no_altera_rutas_fuera_de_directorio_temporal_y_limpia_al_salir() {
    let ruta_capturada = {
        let bd_temp = BaseDatosTemporal::nueva("aislamiento");
        let ruta = bd_temp.ruta().to_path_buf();
        sembrar_dataset_200_turnos(&ruta);
        assert!(ruta.exists(), "la BD temporal debe existir durante el test");
        assert!(ruta.starts_with(std::env::temp_dir()));
        ruta
    };

    assert!(
        !ruta_capturada.exists(),
        "el guardián RAII debe eliminar el archivo temporal al finalizar"
    );
}
