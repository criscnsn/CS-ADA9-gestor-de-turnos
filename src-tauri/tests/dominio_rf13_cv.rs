//! Pruebas de `RF-13` y suite transversal `CV-01`–`CV-06`, `CV-10` (`#10`).
//!
//! Usan el dominio real (`registrar_turno`, `asignar_siguiente`,
//! `finalizar_atencion`, `cancelar_turno` de `#7` y `#9`), sin dobles.

use VentanillaId::{V01, V02, V03};
use gestor_de_turnos_lib::domain::SistemaTurnos;
use gestor_de_turnos_lib::models::{AppError, CodigoError, EstadoTurno, VentanillaId};

/// Sistema nuevo con `n` turnos registrados (`T-0001`, `T-0002`, ...).
fn sistema_con_turnos(n: usize) -> SistemaTurnos {
    let mut sistema = SistemaTurnos::nuevo();
    for _ in 0..n {
        sistema.registrar_turno().expect("registrar turno");
    }
    sistema
}

fn estado(sistema: &SistemaTurnos, turno_id: &str) -> EstadoTurno {
    sistema.buscar_turno(turno_id).expect("turno existe").estado
}

fn ids_en_fila(sistema: &SistemaTurnos) -> Vec<String> {
    sistema.consultar_fila().into_iter().map(|t| t.id).collect()
}

/// Ejecuta `operacion`, exige el rechazo `codigo` y que el estado no cambie (`RNF-R02`).
fn assert_rechazo<T: std::fmt::Debug>(
    sistema: &mut SistemaTurnos,
    codigo: CodigoError,
    operacion: impl FnOnce(&mut SistemaTurnos) -> Result<T, AppError>,
) {
    let antes = sistema.clone();
    let err = operacion(sistema).expect_err("la operación debe rechazarse");
    assert_eq!(err.codigo, codigo, "{}", err.mensaje);
    assert_eq!(*sistema, antes, "un rechazo no altera el estado");
}

// --- RF-13 ---------------------------------------------------------------

#[test]
fn rf13_inhabilitar_dos_libres_se_permite_y_la_tercera_se_bloquea() {
    let mut sistema = SistemaTurnos::nuevo();

    let v01 = sistema
        .cambiar_estado_ventanilla(V01, false)
        .expect("V01 libre");
    assert!(!v01.habilitada);
    sistema
        .cambiar_estado_ventanilla(V02, false)
        .expect("V02 libre");

    assert_rechazo(&mut sistema, CodigoError::UltimaVentanillaActiva, |s| {
        s.cambiar_estado_ventanilla(V03, false)
    });
    assert!(sistema.ventanilla(V03).habilitada);
}

#[test]
fn rf13_inhabilitar_ventanilla_ocupada_se_bloquea() {
    let mut sistema = sistema_con_turnos(1);
    sistema.asignar_siguiente(V02).expect("T-0001 en V02");

    assert_rechazo(&mut sistema, CodigoError::VentanillaOcupada, |s| {
        s.cambiar_estado_ventanilla(V02, false)
    });

    // Ocupada y además última activa: manda el turno en atención.
    sistema
        .cambiar_estado_ventanilla(V01, false)
        .expect("V01 libre");
    sistema
        .cambiar_estado_ventanilla(V03, false)
        .expect("V03 libre");
    assert_rechazo(&mut sistema, CodigoError::VentanillaOcupada, |s| {
        s.cambiar_estado_ventanilla(V02, false)
    });
}

#[test]
fn rf13_ventanilla_inhabilitada_no_recibe_turnos_y_al_rehabilitar_si() {
    let mut sistema = sistema_con_turnos(1);
    sistema
        .cambiar_estado_ventanilla(V01, false)
        .expect("inhabilitar V01");

    assert_rechazo(&mut sistema, CodigoError::VentanillaInhabilitada, |s| {
        s.asignar_siguiente(V01)
    });

    let v01 = sistema
        .cambiar_estado_ventanilla(V01, true)
        .expect("habilitar V01");
    assert!(v01.habilitada);
    assert_eq!(
        sistema.asignar_siguiente(V01).expect("asignar").id,
        "T-0001"
    );
}

#[test]
fn rf13_repetir_el_estado_actual_no_falla_ni_cambia_nada() {
    let mut sistema = SistemaTurnos::nuevo();
    sistema
        .cambiar_estado_ventanilla(V01, true)
        .expect("ya habilitada");
    assert_eq!(sistema, SistemaTurnos::nuevo());

    sistema
        .cambiar_estado_ventanilla(V01, false)
        .expect("inhabilitar");
    let antes = sistema.clone();
    sistema
        .cambiar_estado_ventanilla(V01, false)
        .expect("ya inhabilitada");
    assert_eq!(sistema, antes);
}

// --- CV ------------------------------------------------------------------

#[test]
fn cv01_asignar_v02_y_luego_v01_respeta_fifo() {
    let mut sistema = sistema_con_turnos(3);

    assert_eq!(sistema.asignar_siguiente(V02).expect("V02").id, "T-0001");
    assert_eq!(sistema.asignar_siguiente(V01).expect("V01").id, "T-0002");

    assert_eq!(
        sistema.ventanilla(V02).turno_actual_id.as_deref(),
        Some("T-0001")
    );
    assert_eq!(
        sistema.ventanilla(V01).turno_actual_id.as_deref(),
        Some("T-0002")
    );
    assert_eq!(ids_en_fila(&sistema), ["T-0003"]);
}

#[test]
fn cv02_asignar_en_ventanilla_ocupada_se_rechaza_sin_cambios() {
    let mut sistema = sistema_con_turnos(3);
    sistema.asignar_siguiente(V02).expect("T-0001 en V02");

    assert_rechazo(&mut sistema, CodigoError::VentanillaOcupada, |s| {
        s.asignar_siguiente(V02)
    });
}

#[test]
fn cv03_con_tres_ventanillas_ocupadas_no_hay_cuarta_atencion() {
    let mut sistema = sistema_con_turnos(4);
    for id in [V01, V02, V03] {
        sistema.asignar_siguiente(id).expect("ventanilla libre");
    }

    for id in [V01, V02, V03] {
        assert_rechazo(&mut sistema, CodigoError::VentanillaOcupada, |s| {
            s.asignar_siguiente(id)
        });
    }
    assert_eq!(estado(&sistema, "T-0004"), EstadoTurno::EnEspera);
}

#[test]
fn cv04_finalizar_v02_libera_y_toma_el_siguiente() {
    let mut sistema = sistema_con_turnos(3);
    sistema.asignar_siguiente(V02).expect("T-0001 en V02");
    sistema.asignar_siguiente(V01).expect("T-0002 en V01");

    let atendido = sistema.finalizar_atencion(V02).expect("finalizar V02");
    assert_eq!(atendido.id, "T-0001");
    assert_eq!(atendido.estado, EstadoTurno::Atendido);
    assert_eq!(sistema.ventanilla(V02).turno_actual_id, None);

    assert_eq!(
        sistema.asignar_siguiente(V02).expect("V02 libre").id,
        "T-0003"
    );
}

#[test]
fn cv05_cancelar_t2_en_espera_y_la_asignacion_lo_omite() {
    let mut sistema = sistema_con_turnos(3);

    let cancelado = sistema.cancelar_turno("T-0002").expect("T-0002 en espera");
    assert_eq!(cancelado.estado, EstadoTurno::Cancelado);

    assert_eq!(sistema.asignar_siguiente(V01).expect("V01").id, "T-0001");
    assert_eq!(sistema.asignar_siguiente(V02).expect("V02").id, "T-0003");
    assert_eq!(estado(&sistema, "T-0002"), EstadoTurno::Cancelado);
}

#[test]
fn cv06_cancelar_turno_no_en_espera_o_id_invalido_se_rechaza_sin_cambios() {
    let mut sistema = sistema_con_turnos(3);
    sistema.asignar_siguiente(V01).expect("T-0001 en atención");
    sistema.asignar_siguiente(V02).expect("T-0002 en V02");
    sistema.finalizar_atencion(V02).expect("T-0002 atendido");
    sistema.cancelar_turno("T-0003").expect("T-0003 cancelado");

    for id in ["T-0001", "T-0002", "T-0003"] {
        assert_rechazo(&mut sistema, CodigoError::EstadoTurnoInvalido, |s| {
            s.cancelar_turno(id)
        });
    }
    for id in ["", "   "] {
        assert_rechazo(&mut sistema, CodigoError::EntradaInvalida, |s| {
            s.cancelar_turno(id)
        });
    }
    assert_rechazo(&mut sistema, CodigoError::TurnoInexistente, |s| {
        s.cancelar_turno("T-9999")
    });
}

#[test]
fn cv10_historial_conserva_atendidos_y_cancelados() {
    let mut sistema = sistema_con_turnos(3);
    sistema.asignar_siguiente(V02).expect("T-0001 en V02");
    sistema.finalizar_atencion(V02).expect("T-0001 atendido");
    sistema.cancelar_turno("T-0002").expect("T-0002 cancelado");

    let historial = sistema.consultar_historial();
    let resumen: Vec<_> = historial
        .iter()
        .map(|t| (t.id.as_str(), t.estado, t.ventanilla_asignada))
        .collect();
    assert_eq!(
        resumen,
        [
            ("T-0001", EstadoTurno::Atendido, Some(V02)),
            ("T-0002", EstadoTurno::Cancelado, None),
            ("T-0003", EstadoTurno::EnEspera, None),
        ]
    );
}
