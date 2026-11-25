use gestor_de_turnos_lib::models::{
    AppError, CodigoError, EstadoSistema, EstadoTurno, TipoVista, Turno, Ventanilla, VentanillaId,
};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};

fn round_trip<T: DeserializeOwned + Serialize>(value: Value) {
    let decoded: T = serde_json::from_value(value.clone()).expect("fixture válido");
    assert_eq!(serde_json::to_value(decoded).unwrap(), value);
}

#[test]
fn snapshots_compartidos_con_typescript_conservan_campos_y_null() {
    for fixture in [
        include_str!("../../docs/fixtures/contrato/estado-vacio.json"),
        include_str!("../../docs/fixtures/contrato/estado-mixto.json"),
    ] {
        let value: Value = serde_json::from_str(fixture).unwrap();
        round_trip::<EstadoSistema>(value.clone());
        for turno in value["historial"].as_array().unwrap() {
            round_trip::<Turno>(turno.clone());
        }
        for ventanilla in value["ventanillas"].as_array().unwrap() {
            round_trip::<Ventanilla>(ventanilla.clone());
        }
    }
}

#[test]
fn todos_los_errores_preservan_codigo_mensaje_y_recurso() {
    let errors: Vec<Value> =
        serde_json::from_str(include_str!("../../docs/fixtures/contrato/errores.json")).unwrap();
    assert_eq!(errors.len(), 11);
    for error in errors {
        round_trip::<AppError>(error);
    }
}

#[test]
fn enumeraciones_serializan_exactamente_como_el_contrato() {
    let values: Value = serde_json::from_str(include_str!(
        "../../docs/fixtures/contrato/enumeraciones.json"
    ))
    .unwrap();
    assert_eq!(
        serde_json::to_value([
            EstadoTurno::EnEspera,
            EstadoTurno::EnAtencion,
            EstadoTurno::Atendido,
            EstadoTurno::Cancelado,
        ])
        .unwrap(),
        values["estadosTurno"]
    );
    assert_eq!(
        serde_json::to_value([VentanillaId::V01, VentanillaId::V02, VentanillaId::V03,]).unwrap(),
        values["ventanillaIds"]
    );
    assert_eq!(
        serde_json::to_value([TipoVista::Cliente, TipoVista::Cajero]).unwrap(),
        values["tiposVista"]
    );
    assert_eq!(
        serde_json::to_value([
            CodigoError::VentanillaInvalida,
            CodigoError::VentanillaOcupada,
            CodigoError::VentanillaLibre,
            CodigoError::VentanillaInhabilitada,
            CodigoError::UltimaVentanillaActiva,
            CodigoError::FilaVacia,
            CodigoError::TurnoInexistente,
            CodigoError::EstadoTurnoInvalido,
            CodigoError::EntradaInvalida,
            CodigoError::ErrorPersistencia,
            CodigoError::BaseDatosCorrupta,
        ])
        .unwrap(),
        values["codigosError"]
    );
    for value in values["estadosTurno"].as_array().unwrap() {
        round_trip::<EstadoTurno>(value.clone());
    }
    for value in values["ventanillaIds"].as_array().unwrap() {
        round_trip::<VentanillaId>(value.clone());
    }
    for value in values["tiposVista"].as_array().unwrap() {
        round_trip::<TipoVista>(value.clone());
    }
    for value in values["codigosError"].as_array().unwrap() {
        round_trip::<CodigoError>(value.clone());
    }
}

#[test]
fn rechaza_enums_desconocidos_y_cantidad_incorrecta_de_ventanillas() {
    assert!(serde_json::from_value::<EstadoTurno>(json!("enEspera")).is_err());
    assert!(serde_json::from_value::<CodigoError>(json!("filaVacia")).is_err());
    assert!(serde_json::from_value::<VentanillaId>(json!("V04")).is_err());
    assert!(serde_json::from_value::<TipoVista>(json!("administrador")).is_err());
    let value: Value = serde_json::from_str(include_str!(
        "../../docs/fixtures/contrato/estado-vacio.json"
    ))
    .unwrap();
    for cantidad in [0, 2, 4] {
        let mut invalid = value.clone();
        invalid["ventanillas"] = Value::Array(vec![value["ventanillas"][0].clone(); cantidad]);
        assert!(serde_json::from_value::<EstadoSistema>(invalid).is_err());
    }
}
