//! Evidencia de cierre de `#14` con SQLite real: comandos (`Sgt`) + `RepositorioSqlite`.
//!
//! - Commit exitoso → se publica la memoria → se emite un evento, y lo
//!   persistido sobrevive a reabrir la base de datos.
//! - Fallo de escritura → memoria intacta, sin evento y sin cambios en disco.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use gestor_de_turnos_lib::comandos::{Sgt, iniciar_sgt_en_ruta};
use gestor_de_turnos_lib::db::DEFAULT_DB_FILENAME;
use gestor_de_turnos_lib::domain::SistemaTurnos;
use gestor_de_turnos_lib::models::{AppError, EstadoSistema, EstadoTurno, VentanillaId};

/// Base de datos SQLite en un directorio temporal propio; se borra al terminar.
struct BdTemporal(PathBuf);

impl BdTemporal {
    fn nueva(etiqueta: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("reloj posterior a UNIX_EPOCH")
            .as_nanos();
        Self(std::env::temp_dir().join(format!(
            "sgt_comandos_{etiqueta}_{}_{nanos}",
            std::process::id()
        )))
    }

    fn abrir(&self) -> Sgt {
        iniciar_sgt_en_ruta(self.0.join(DEFAULT_DB_FILENAME)).expect("abrir SQLite real")
    }
}

impl Drop for BdTemporal {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Ejecuta `operacion` y devuelve su resultado junto a los eventos emitidos.
fn ejecutar<T>(
    sgt: &mut Sgt,
    operacion: impl FnOnce(&mut SistemaTurnos) -> Result<T, AppError>,
) -> (Result<T, AppError>, Vec<EstadoSistema>) {
    let mut eventos = Vec::new();
    let resultado = sgt.ejecutar(operacion, |snapshot| eventos.push(snapshot));
    (resultado, eventos)
}

#[test]
fn commit_exitoso_publica_memoria_emite_evento_y_persiste_en_disco() {
    let bd = BdTemporal::nueva("exito");
    let mut sgt = bd.abrir();

    let (turno, eventos) = ejecutar(&mut sgt, SistemaTurnos::registrar_turno);
    let turno = turno.expect("registrar");
    assert_eq!(
        sgt.obtener_estado().fila_espera,
        std::slice::from_ref(&turno)
    );
    assert_eq!(eventos, [sgt.obtener_estado()]);

    // Recorrido completo: el turno pasa a ATENDIDO y luego se modifica solo una
    // ventanilla. En SQLite real, reenviar un turno terminal haría fallar el trigger.
    for paso in [
        |s: &mut SistemaTurnos| s.asignar_siguiente(VentanillaId::V01).map(drop),
        |s: &mut SistemaTurnos| s.finalizar_atencion(VentanillaId::V01).map(drop),
        |s: &mut SistemaTurnos| {
            s.cambiar_estado_ventanilla(VentanillaId::V03, false)
                .map(drop)
        },
    ] {
        let (resultado, eventos) = ejecutar(&mut sgt, paso);
        resultado.expect("operación confirmada en SQLite");
        assert_eq!(eventos.len(), 1, "un evento por commit");
    }

    let en_memoria = sgt.obtener_estado();
    assert_eq!(en_memoria.historial[0].estado, EstadoTurno::Atendido);
    drop(sgt);

    assert_eq!(
        bd.abrir().obtener_estado(),
        en_memoria,
        "lo publicado está en disco"
    );
}

#[cfg(feature = "test-fault-injection")]
#[test]
fn fallo_de_escritura_deja_memoria_intacta_sin_evento_y_sin_cambios_en_disco() {
    use gestor_de_turnos_lib::models::CodigoError;
    use gestor_de_turnos_lib::repository::PuntoFalloEscritura;

    for punto in [
        PuntoFalloEscritura::DespuesDeTurnos,
        PuntoFalloEscritura::DespuesDeVentanillas,
        PuntoFalloEscritura::AntesDeCommit,
    ] {
        let bd = BdTemporal::nueva("fallo");
        let mut sgt = bd.abrir();
        ejecutar(&mut sgt, SistemaTurnos::registrar_turno)
            .0
            .expect("T-0001");
        let antes = sgt.obtener_estado();

        sgt.inyectar_fallo_escritura(punto);
        let (resultado, eventos) = ejecutar(&mut sgt, |s| s.asignar_siguiente(VentanillaId::V01));

        let err = resultado.expect_err("la escritura debe fallar");
        assert_eq!(err.codigo, CodigoError::ErrorPersistencia, "{punto:?}");
        assert_eq!(sgt.obtener_estado(), antes, "memoria intacta en {punto:?}");
        assert!(eventos.is_empty(), "sin evento en {punto:?}");
        drop(sgt);

        assert_eq!(
            bd.abrir().obtener_estado(),
            antes,
            "sin cambios en disco en {punto:?}"
        );
    }
}
