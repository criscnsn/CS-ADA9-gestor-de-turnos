use chrono::{TimeZone, Utc};
use gestor_de_turnos_lib::domain::{MAX_SECUENCIA_JS, SistemaTurnos, formatear_id_turno};
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

#[test]
fn estado_inicial_tiene_tres_ventanillas_habilitadas_y_sin_turnos() {
    let sistema = SistemaTurnos::nuevo();

    assert_eq!(sistema.ultima_secuencia(), 0);
    assert_eq!(sistema.consultar_ventanillas().len(), 3);
    assert_eq!(sistema.ventanilla(VentanillaId::V01).id, VentanillaId::V01);
    assert!(sistema.ventanilla(VentanillaId::V01).habilitada);
    assert_eq!(sistema.ventanilla(VentanillaId::V01).turno_actual_id, None);
    assert_eq!(sistema.obtener_estado(), fixture_vacio());
}

#[test]
fn registrar_turnos_genera_secuencia_monotonica_id_unico_fecha_iso_y_estado_en_espera() {
    let mut sistema = SistemaTurnos::nuevo();
    let instante = Utc
        .with_ymd_and_hms(2026, 11, 18, 1, 0, 0)
        .single()
        .expect("fecha UTC válida");

    let t1 = sistema
        .registrar_turno_en(instante)
        .expect("debe registrar T-0001");
    let t2 = sistema
        .registrar_turno_en(instante)
        .expect("debe registrar T-0002");
    let t3 = sistema
        .registrar_turno()
        .expect("debe registrar T-0003 con reloj UTC");

    assert_eq!(t1.id, "T-0001");
    assert_eq!(t1.secuencia, 1);
    assert_eq!(t1.fecha_hora_registro, "2026-11-18T01:00:00.000Z");
    assert_eq!(t1.estado, EstadoTurno::EnEspera);
    assert_eq!(t1.ventanilla_asignada, None);

    assert_eq!(t2.id, "T-0002");
    assert_eq!(t2.secuencia, 2);
    assert_eq!(t2.fecha_hora_registro, "2026-11-18T01:00:00.000Z");
    assert_eq!(t2.estado, EstadoTurno::EnEspera);
    assert_eq!(t2.ventanilla_asignada, None);

    assert_eq!(t3.id, "T-0003");
    assert_eq!(t3.secuencia, 3);
    assert!(t3.fecha_hora_registro.ends_with('Z'));
    assert!(chrono::DateTime::parse_from_rfc3339(&t3.fecha_hora_registro).is_ok());

    assert_eq!(sistema.ultima_secuencia(), 3);
    assert_eq!(
        sistema
            .consultar_fila()
            .iter()
            .map(|t| t.secuencia)
            .collect::<Vec<_>>(),
        vec![1, 2, 3]
    );
    assert_eq!(sistema.consultar_historial().len(), 3);
    assert_eq!(sistema.buscar_turno("T-0002"), Some(&t2));
    assert_eq!(sistema.buscar_turno("T-9999"), None);
}

#[test]
fn consulta_de_fila_e_historial_desde_fixture_mixto_respeta_estados_y_orden_por_secuencia() {
    let fixture = fixture_mixto();
    let sistema = SistemaTurnos::desde_snapshot(fixture.clone(), None)
        .expect("debe restaurar el fixture estado-mixto.json");

    // RF-01: ventanillas preservan su estado exacto.
    assert_eq!(
        sistema
            .ventanilla(VentanillaId::V01)
            .turno_actual_id
            .as_deref(),
        Some("T-0003")
    );
    assert_eq!(sistema.ventanilla(VentanillaId::V02).turno_actual_id, None);
    assert!(!sistema.ventanilla(VentanillaId::V03).habilitada);

    // RF-03: fila solo contiene turnos EN_ESPERA.
    let fila = sistema.consultar_fila();
    assert_eq!(fila.len(), 1);
    assert_eq!(fila[0].id, "T-0004");
    assert_eq!(fila[0].estado, EstadoTurno::EnEspera);

    // RF-10: historial conserva los 4 estados.
    let historial = sistema.consultar_historial();
    assert_eq!(historial.len(), 4);
    assert_eq!(
        historial.iter().map(|t| t.estado).collect::<Vec<_>>(),
        vec![
            EstadoTurno::Atendido,
            EstadoTurno::Cancelado,
            EstadoTurno::EnAtencion,
            EstadoTurno::EnEspera,
        ]
    );

    // El snapshot regenerado coincide exactamente con el fixture del contrato.
    assert_eq!(sistema.obtener_estado(), fixture);
    assert_eq!(sistema.ultima_secuencia(), 4);
}

#[test]
fn fila_se_ordena_por_secuencia_numerica_y_no_por_id_alfabetico_ni_reloj() {
    let ventanillas = [
        Ventanilla {
            id: VentanillaId::V03,
            habilitada: true,
            turno_actual_id: None,
        },
        Ventanilla {
            id: VentanillaId::V01,
            habilitada: true,
            turno_actual_id: None,
        },
        Ventanilla {
            id: VentanillaId::V02,
            habilitada: true,
            turno_actual_id: None,
        },
    ];

    // Historial desordenado intencionalmente y con marcas de tiempo invertidas
    // para comprobar que manda `secuencia` (RN-02) y no el reloj ni el orden léxico.
    let turnos = vec![
        Turno {
            id: formatear_id_turno(10_000),
            secuencia: 10_000,
            fecha_hora_registro: "2026-11-18T00:00:01.000Z".into(),
            estado: EstadoTurno::EnEspera,
            ventanilla_asignada: None,
        },
        Turno {
            id: formatear_id_turno(2),
            secuencia: 2,
            fecha_hora_registro: "2026-11-18T00:00:05.000Z".into(),
            estado: EstadoTurno::EnEspera,
            ventanilla_asignada: None,
        },
        Turno {
            id: formatear_id_turno(10),
            secuencia: 10,
            fecha_hora_registro: "2026-11-18T00:00:03.000Z".into(),
            estado: EstadoTurno::Cancelado,
            ventanilla_asignada: None,
        },
        Turno {
            id: formatear_id_turno(9),
            secuencia: 9,
            fecha_hora_registro: "2026-11-18T00:00:04.000Z".into(),
            estado: EstadoTurno::EnEspera,
            ventanilla_asignada: None,
        },
    ];

    let sistema = SistemaTurnos::restaurar(ventanillas, turnos, 10_000)
        .expect("debe normalizar ventanillas y restaurar turnos");

    // Ventanillas normalizadas en orden V01, V02, V03.
    assert_eq!(
        sistema.consultar_ventanillas().each_ref().map(|v| v.id),
        [VentanillaId::V01, VentanillaId::V02, VentanillaId::V03]
    );

    // Solo los 3 EN_ESPERA en orden numérico 2, 9, 10000 (no alfabético T-10000 antes de T-0009).
    let fila = sistema.consultar_fila();
    assert_eq!(
        fila.iter()
            .map(|t| (t.id.as_str(), t.secuencia))
            .collect::<Vec<_>>(),
        vec![("T-0002", 2), ("T-0009", 9), ("T-10000", 10_000)]
    );

    // Historial conserva los 4 turnos ordenados por secuencia.
    assert_eq!(
        sistema
            .consultar_historial()
            .iter()
            .map(|t| t.secuencia)
            .collect::<Vec<_>>(),
        vec![2, 9, 10, 10_000]
    );
}

#[test]
fn secuencia_restaurada_desde_repositorio_no_reutiliza_ids_y_coordina_con_transaccion() {
    // Simula un repositorio donde la tabla `secuencias` ya va en 10 aunque el
    // último turno visible en el fixture sea 4 (CV-08).
    let mut sistema = SistemaTurnos::desde_snapshot(fixture_mixto(), Some(10))
        .expect("debe aceptar ultima_secuencia mayor al máximo del historial");

    let instante = Utc
        .with_ymd_and_hms(2026, 11, 25, 19, 30, 0)
        .single()
        .expect("fecha UTC válida");

    let t11 = sistema
        .registrar_turno_en(instante)
        .expect("el siguiente turno debe ser 11");
    assert_eq!(t11.secuencia, 11);
    assert_eq!(t11.id, "T-0011");

    // Simula secuencia asignada por la transacción de #12 (p. ej. tras salto a 12).
    let t12 = sistema
        .registrar_turno_con_secuencia(12, instante)
        .expect("debe aceptar secuencia estrictamente mayor del repositorio");
    assert_eq!(t12.secuencia, 12);
    assert_eq!(t12.id, "T-0012");
    assert_eq!(sistema.ultima_secuencia(), 12);

    // Rechaza una secuencia repetida o retrocedida sin alterar el estado.
    let estado_previo = sistema.obtener_estado();
    let err = sistema
        .registrar_turno_con_secuencia(12, instante)
        .expect_err("no debe permitir reutilizar secuencia");
    assert_eq!(err.codigo, CodigoError::EntradaInvalida);
    assert_eq!(sistema.obtener_estado(), estado_previo);
}

#[test]
fn restauracion_y_registro_rechazan_datos_corruptos_o_fuera_de_rango() {
    let base = fixture_vacio();

    // Ventanillas duplicadas.
    let mut ventanillas_duplicadas = base.ventanillas.clone();
    ventanillas_duplicadas[1].id = VentanillaId::V01;
    let err = SistemaTurnos::restaurar(ventanillas_duplicadas, vec![], 0)
        .expect_err("debe rechazar ventanillas duplicadas");
    assert_eq!(err.codigo, CodigoError::BaseDatosCorrupta);

    // Turno con secuencia mayor a ultima_secuencia persistida.
    let err = SistemaTurnos::desde_snapshot(fixture_mixto(), Some(2))
        .expect_err("debe rechazar secuencia persistida menor al historial");
    assert_eq!(err.codigo, CodigoError::BaseDatosCorrupta);

    // Turno con secuencia 0.
    let turno_cero = Turno {
        id: "T-0000".into(),
        secuencia: 0,
        fecha_hora_registro: "2026-11-18T01:00:00.000Z".into(),
        estado: EstadoTurno::EnEspera,
        ventanilla_asignada: None,
    };
    let err = SistemaTurnos::restaurar(base.ventanillas.clone(), vec![turno_cero], 1)
        .expect_err("debe rechazar secuencia 0");
    assert_eq!(err.codigo, CodigoError::BaseDatosCorrupta);

    // Límite seguro de secuencia JS (MAX_SECUENCIA_JS).
    let mut en_limite =
        SistemaTurnos::restaurar(base.ventanillas.clone(), vec![], MAX_SECUENCIA_JS)
            .expect("debe permitir restaurar en MAX_SECUENCIA_JS");
    let err = en_limite
        .registrar_turno()
        .expect_err("debe rechazar superar MAX_SECUENCIA_JS");
    assert_eq!(err.codigo, CodigoError::EntradaInvalida);

    // Todas las ventanillas inhabilitadas (viola RF-13).
    let mut todas_inhabilitadas = base.ventanillas.clone();
    for v in &mut todas_inhabilitadas {
        v.habilitada = false;
    }
    let err = SistemaTurnos::restaurar(todas_inhabilitadas, vec![], 0)
        .expect_err("debe rechazar restaurar sin ninguna ventanilla activa");
    assert_eq!(err.codigo, CodigoError::BaseDatosCorrupta);

    // Ventanilla apuntando a un turno inexistente.
    let mut ventanilla_huerfana = base.ventanillas.clone();
    ventanilla_huerfana[0].turno_actual_id = Some("T-9999".into());
    let err = SistemaTurnos::restaurar(ventanilla_huerfana, vec![], 9999)
        .expect_err("debe rechazar referencia a turno inexistente");
    assert_eq!(err.codigo, CodigoError::BaseDatosCorrupta);

    // Turno EN_ESPERA con ventanilla asignada.
    let turno_inconsistente = Turno {
        id: "T-0001".into(),
        secuencia: 1,
        fecha_hora_registro: "2026-11-18T01:00:00.000Z".into(),
        estado: EstadoTurno::EnEspera,
        ventanilla_asignada: Some(VentanillaId::V01),
    };
    let err = SistemaTurnos::restaurar(base.ventanillas, vec![turno_inconsistente], 1)
        .expect_err("debe rechazar EN_ESPERA con ventanilla asignada");
    assert_eq!(err.codigo, CodigoError::BaseDatosCorrupta);
}
