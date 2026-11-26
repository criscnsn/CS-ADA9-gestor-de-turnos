use chrono::{TimeZone, Utc};
use gestor_de_turnos_lib::domain::SistemaTurnos;
use gestor_de_turnos_lib::models::{
    CodigoError, EstadoSistema, EstadoTurno, Turno, Ventanilla, VentanillaId,
};

fn fixture_vacio() -> EstadoSistema {
    serde_json::from_str(include_str!(
        "../../docs/fixtures/contrato/estado-vacio.json"
    ))
    .expect("el fixture estado-vacio.json debe deserializar")
}

fn fixture_mixto() -> EstadoSistema {
    serde_json::from_str(include_str!(
        "../../docs/fixtures/contrato/estado-mixto.json"
    ))
    .expect("el fixture estado-mixto.json debe deserializar")
}

fn ventanillas_libres() -> [Ventanilla; 3] {
    fixture_vacio().ventanillas
}

fn turno_en_espera(id: &str, secuencia: u64, fecha: &str) -> Turno {
    Turno {
        id: id.into(),
        secuencia,
        fecha_hora_registro: fecha.into(),
        estado: EstadoTurno::EnEspera,
        ventanilla_asignada: None,
    }
}

#[test]
fn cv01_asignar_siguiente_respeta_orden_fifo_por_secuencia_en_cualquier_ventanilla() {
    // Fixture preparado sin usar registrar_turno: T-0001, T-0002 y T-0003 en espera.
    let mut sistema = SistemaTurnos::restaurar(
        ventanillas_libres(),
        vec![
            turno_en_espera("T-0003", 3, "2026-11-18T01:00:00.000Z"),
            turno_en_espera("T-0001", 1, "2026-11-18T01:05:00.000Z"),
            turno_en_espera("T-0002", 2, "2026-11-18T01:02:00.000Z"),
        ],
        3,
    )
    .expect("fixture de 3 turnos en espera válido");

    let asignado_v02 = sistema
        .asignar_siguiente(VentanillaId::V02)
        .expect("V02 debe recibir el turno de menor secuencia (T-0001)");
    assert_eq!(asignado_v02.id, "T-0001");
    assert_eq!(asignado_v02.estado, EstadoTurno::EnAtencion);
    assert_eq!(asignado_v02.ventanilla_asignada, Some(VentanillaId::V02));
    assert_eq!(
        sistema
            .ventanilla(VentanillaId::V02)
            .turno_actual_id
            .as_deref(),
        Some("T-0001")
    );

    let asignado_v01 = sistema
        .asignar_siguiente(VentanillaId::V01)
        .expect("V01 debe recibir el siguiente turno FIFO (T-0002)");
    assert_eq!(asignado_v01.id, "T-0002");
    assert_eq!(asignado_v01.estado, EstadoTurno::EnAtencion);
    assert_eq!(asignado_v01.ventanilla_asignada, Some(VentanillaId::V01));
    assert_eq!(
        sistema
            .ventanilla(VentanillaId::V01)
            .turno_actual_id
            .as_deref(),
        Some("T-0002")
    );

    // T-0003 permanece EnEspera y V03 sigue libre.
    assert_eq!(sistema.ventanilla(VentanillaId::V03).turno_actual_id, None);
    let fila = sistema.consultar_fila();
    assert_eq!(fila.len(), 1);
    assert_eq!(fila[0].id, "T-0003");
}

#[test]
fn cv02_y_rf13_asignar_rechaza_ventanilla_ocupada_inhabilitada_y_fila_vacia_sin_mutar() {
    // En estado-mixto.json: V01 ocupada (T-0003), V02 libre, V03 inhabilitada, T-0004 en espera.
    let mut sistema = SistemaTurnos::desde_snapshot(fixture_mixto(), None)
        .expect("debe restaurar estado-mixto.json");
    let estado_inicial = sistema.obtener_estado();

    // CV-02: V01 ocupada.
    let err_ocupada = sistema
        .asignar_siguiente(VentanillaId::V01)
        .expect_err("V01 ocupada debe rechazar asignación");
    assert_eq!(err_ocupada.codigo, CodigoError::VentanillaOcupada);
    assert_eq!(err_ocupada.recurso_id.as_deref(), Some("V01"));
    assert!(err_ocupada.mensaje.contains("V01"));
    assert!(err_ocupada.mensaje.contains("T-0003"));
    assert_eq!(sistema.obtener_estado(), estado_inicial);

    // Guard RF-13: V03 inhabilitada.
    let err_inhabilitada = sistema
        .asignar_siguiente(VentanillaId::V03)
        .expect_err("V03 inhabilitada debe rechazar asignación");
    assert_eq!(err_inhabilitada.codigo, CodigoError::VentanillaInhabilitada);
    assert_eq!(err_inhabilitada.recurso_id.as_deref(), Some("V03"));
    assert!(err_inhabilitada.mensaje.contains("V03"));
    assert_eq!(sistema.obtener_estado(), estado_inicial);

    // RF-05 / RN-10: fila vacía sobre estado-vacio.json.
    let mut vacio = SistemaTurnos::desde_snapshot(fixture_vacio(), None)
        .expect("debe restaurar estado-vacio.json");
    let vacio_inicial = vacio.obtener_estado();
    let err_vacia = vacio
        .asignar_siguiente(VentanillaId::V02)
        .expect_err("fila vacía debe rechazar asignación");
    assert_eq!(err_vacia.codigo, CodigoError::FilaVacia);
    assert_eq!(err_vacia.recurso_id.as_deref(), Some("V02"));
    assert!(err_vacia.mensaje.contains("V02"));
    assert_eq!(vacio.obtener_estado(), vacio_inicial);
}

#[test]
fn cv03_con_tres_ventanillas_ocupadas_no_permite_cuarta_atencion_y_conserva_fila() {
    let mut sistema = SistemaTurnos::restaurar(
        [
            Ventanilla {
                id: VentanillaId::V01,
                habilitada: true,
                turno_actual_id: Some("T-0001".into()),
            },
            Ventanilla {
                id: VentanillaId::V02,
                habilitada: true,
                turno_actual_id: Some("T-0002".into()),
            },
            Ventanilla {
                id: VentanillaId::V03,
                habilitada: true,
                turno_actual_id: Some("T-0003".into()),
            },
        ],
        vec![
            Turno {
                id: "T-0001".into(),
                secuencia: 1,
                fecha_hora_registro: "2026-11-18T01:00:00.000Z".into(),
                estado: EstadoTurno::EnAtencion,
                ventanilla_asignada: Some(VentanillaId::V01),
            },
            Turno {
                id: "T-0002".into(),
                secuencia: 2,
                fecha_hora_registro: "2026-11-18T01:01:00.000Z".into(),
                estado: EstadoTurno::EnAtencion,
                ventanilla_asignada: Some(VentanillaId::V02),
            },
            Turno {
                id: "T-0003".into(),
                secuencia: 3,
                fecha_hora_registro: "2026-11-18T01:02:00.000Z".into(),
                estado: EstadoTurno::EnAtencion,
                ventanilla_asignada: Some(VentanillaId::V03),
            },
            turno_en_espera("T-0004", 4, "2026-11-18T01:03:00.000Z"),
        ],
        4,
    )
    .expect("fixture con 3 ventanillas ocupadas válido");
    let estado_inicial = sistema.obtener_estado();

    for vid in [VentanillaId::V01, VentanillaId::V02, VentanillaId::V03] {
        let err = sistema
            .asignar_siguiente(vid)
            .expect_err("ninguna ventanilla ocupada debe aceptar un cuarto turno");
        assert_eq!(err.codigo, CodigoError::VentanillaOcupada);
    }

    assert_eq!(sistema.obtener_estado(), estado_inicial);
    assert_eq!(sistema.consultar_fila().len(), 1);
    assert_eq!(sistema.consultar_fila()[0].id, "T-0004");
}

#[test]
fn cv04_y_rf07_finalizar_libera_ventanilla_conserva_historial_y_rechaza_ventanilla_libre() {
    let mut sistema = SistemaTurnos::desde_snapshot(fixture_mixto(), None)
        .expect("debe restaurar estado-mixto.json");

    // RF-07: rechazar finalización sobre ventanilla libre (V02) e inhabilitada libre (V03).
    let estado_antes = sistema.obtener_estado();
    let err_libre = sistema
        .finalizar_atencion(VentanillaId::V02)
        .expect_err("V02 libre debe rechazar finalización");
    assert_eq!(err_libre.codigo, CodigoError::VentanillaLibre);
    assert_eq!(err_libre.recurso_id.as_deref(), Some("V02"));
    assert!(err_libre.mensaje.contains("V02"));
    assert_eq!(sistema.obtener_estado(), estado_antes);

    let err_inhabilitada_libre = sistema
        .finalizar_atencion(VentanillaId::V03)
        .expect_err("V03 libre e inhabilitada debe rechazar finalización");
    assert_eq!(err_inhabilitada_libre.codigo, CodigoError::VentanillaLibre);
    assert_eq!(err_inhabilitada_libre.recurso_id.as_deref(), Some("V03"));
    assert_eq!(sistema.obtener_estado(), estado_antes);

    // CV-04: finalizar T-0003 en V01 y volver a asignar desde V01.
    let finalizado = sistema
        .finalizar_atencion(VentanillaId::V01)
        .expect("debe finalizar T-0003 en V01");
    assert_eq!(finalizado.id, "T-0003");
    assert_eq!(finalizado.estado, EstadoTurno::Atendido);
    assert_eq!(finalizado.ventanilla_asignada, Some(VentanillaId::V01));
    assert_eq!(sistema.ventanilla(VentanillaId::V01).turno_actual_id, None);

    let siguiente = sistema
        .asignar_siguiente(VentanillaId::V01)
        .expect("V01 liberada debe recibir T-0004");
    assert_eq!(siguiente.id, "T-0004");
    assert_eq!(siguiente.estado, EstadoTurno::EnAtencion);
    assert_eq!(siguiente.ventanilla_asignada, Some(VentanillaId::V01));
    assert!(sistema.consultar_fila().is_empty());

    // Historial conserva T-0003 como Atendido con su ventanilla V01.
    let t3 = sistema
        .buscar_turno("T-0003")
        .expect("T-0003 debe permanecer en historial");
    assert_eq!(t3.estado, EstadoTurno::Atendido);
    assert_eq!(t3.ventanilla_asignada, Some(VentanillaId::V01));
}

#[test]
fn cv05_cancelar_turno_en_espera_lo_retira_de_fila_lo_conserva_en_historial_y_se_omite_al_asignar()
{
    let mut sistema = SistemaTurnos::restaurar(
        ventanillas_libres(),
        vec![
            turno_en_espera("T-0001", 1, "2026-11-18T01:00:00.000Z"),
            turno_en_espera("T-0002", 2, "2026-11-18T01:01:00.000Z"),
            turno_en_espera("T-0003", 3, "2026-11-18T01:02:00.000Z"),
        ],
        3,
    )
    .expect("fixture de 3 turnos en espera válido");

    // También valida trim de espacios laterales en el identificador.
    let cancelado = sistema
        .cancelar_turno("  T-0002  ")
        .expect("debe cancelar T-0002 en espera");
    assert_eq!(cancelado.id, "T-0002");
    assert_eq!(cancelado.estado, EstadoTurno::Cancelado);
    assert_eq!(cancelado.ventanilla_asignada, None);

    assert_eq!(
        sistema
            .consultar_fila()
            .iter()
            .map(|t| t.id.as_str())
            .collect::<Vec<_>>(),
        vec!["T-0001", "T-0003"]
    );
    assert_eq!(sistema.consultar_historial().len(), 3);
    assert_eq!(
        sistema.buscar_turno("T-0002").map(|t| t.estado),
        Some(EstadoTurno::Cancelado)
    );

    let primero = sistema
        .asignar_siguiente(VentanillaId::V01)
        .expect("debe asignar T-0001");
    let segundo = sistema
        .asignar_siguiente(VentanillaId::V02)
        .expect("debe omitir T-0002 y asignar T-0003");
    assert_eq!(primero.id, "T-0001");
    assert_eq!(segundo.id, "T-0003");
}

#[test]
fn cv06_y_rf09_cancelar_rechaza_entradas_invalidas_inexistentes_y_estados_no_espera_sin_mutar() {
    let mut sistema = SistemaTurnos::desde_snapshot(fixture_mixto(), None)
        .expect("debe restaurar estado-mixto.json");
    let estado_inicial = sistema.obtener_estado();

    // Entradas vacías o malformadas (RNF-S04).
    for entrada in ["", "   ", "T-0004\n", "\t"] {
        let err = sistema
            .cancelar_turno(entrada)
            .expect_err("debe rechazar entrada vacía o con caracteres de control");
        assert_eq!(err.codigo, CodigoError::EntradaInvalida);
        assert_eq!(err.recurso_id, None);
    }

    // ID inexistente (RF-09).
    let err_inexistente = sistema
        .cancelar_turno("T-9999")
        .expect_err("debe rechazar ID inexistente");
    assert_eq!(err_inexistente.codigo, CodigoError::TurnoInexistente);
    assert_eq!(err_inexistente.recurso_id.as_deref(), Some("T-9999"));
    assert!(err_inexistente.mensaje.contains("T-9999"));

    // CV-06: turno EnAtencion (T-0003), Atendido (T-0001) y Cancelado (T-0002).
    for id in ["T-0003", "T-0001", "T-0002"] {
        let err = sistema
            .cancelar_turno(id)
            .expect_err("no debe permitir cancelar turno fuera de EnEspera");
        assert_eq!(err.codigo, CodigoError::EstadoTurnoInvalido);
        assert_eq!(err.recurso_id.as_deref(), Some(id));
        assert!(err.mensaje.contains(id));
    }

    // Integridad ante rechazos (RNF-R02): ningún dato ni ventanilla cambió.
    assert_eq!(sistema.obtener_estado(), estado_inicial);
}

#[test]
fn flujo_conjunto_con_registro_de_3a_y_rechazo_de_entrada_malformada_rnf_s04() {
    let mut sistema = SistemaTurnos::nuevo();
    let instante = Utc
        .with_ymd_and_hms(2026, 11, 25, 20, 30, 0)
        .single()
        .expect("fecha UTC válida");

    let t1 = sistema
        .registrar_turno_en(instante)
        .expect("debe registrar T-0001");
    let t2 = sistema
        .registrar_turno_en(instante)
        .expect("debe registrar T-0002");
    let t3 = sistema
        .registrar_turno_en(instante)
        .expect("debe registrar T-0003");

    // RNF-S04: una entrada malformada tipo inyección SQL se rechaza sin mutar el estado.
    let estado_antes_inyeccion = sistema.obtener_estado();
    let err_sql = sistema
        .cancelar_turno("'; DROP TABLE turnos; --")
        .expect_err("debe rechazar identificador malformado inexistente");
    assert_eq!(err_sql.codigo, CodigoError::TurnoInexistente);
    assert_eq!(
        err_sql.recurso_id.as_deref(),
        Some("'; DROP TABLE turnos; --")
    );
    assert_eq!(sistema.obtener_estado(), estado_antes_inyeccion);

    // Flujo conjunto completo: asignar T-0001, cancelar T-0002, finalizar T-0001 y asignar T-0003.
    let en_v02 = sistema
        .asignar_siguiente(VentanillaId::V02)
        .expect("debe asignar T-0001 a V02");
    assert_eq!(en_v02.id, t1.id);

    let cancelado = sistema
        .cancelar_turno(&t2.id)
        .expect("debe cancelar T-0002 en espera");
    assert_eq!(cancelado.estado, EstadoTurno::Cancelado);

    let atendido = sistema
        .finalizar_atencion(VentanillaId::V02)
        .expect("debe finalizar T-0001 en V02");
    assert_eq!(atendido.estado, EstadoTurno::Atendido);

    let en_v01 = sistema
        .asignar_siguiente(VentanillaId::V01)
        .expect("debe omitir T-0002 cancelado y asignar T-0003 a V01");
    assert_eq!(en_v01.id, t3.id);

    let estado = sistema.obtener_estado();
    assert!(estado.fila_espera.is_empty());
    assert_eq!(estado.historial.len(), 3);
    assert_eq!(
        estado
            .historial
            .iter()
            .map(|t| (t.id.as_str(), t.estado))
            .collect::<Vec<_>>(),
        vec![
            ("T-0001", EstadoTurno::Atendido),
            ("T-0002", EstadoTurno::Cancelado),
            ("T-0003", EstadoTurno::EnAtencion),
        ]
    );
}
