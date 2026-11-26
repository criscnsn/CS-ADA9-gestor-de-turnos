//! Pruebas de integración del gestor multi-ventana (`#07-A`).

use std::collections::HashMap;
use std::sync::Mutex;
use std::thread;

use gestor_de_turnos_lib::models::{CodigoError, TipoVista};
use gestor_de_turnos_lib::windows::{
    ConfiguracionVentana, ETIQUETA_VENTANA_CAJERO, EntornoVentanas, GestorVentanas,
    PREFIJO_VENTANA_CLIENTE, RUTA_VISTA_CAJERO, RUTA_VISTA_CLIENTE, ResultadoVentana,
    parsear_tipo_vista,
};
use serde_json::json;

#[derive(Debug, Clone)]
struct VentanaSimulada {
    config: ConfiguracionVentana,
    enfoques: usize,
}

#[derive(Debug, Default)]
struct EntornoSimulado {
    ventanas: Mutex<HashMap<String, VentanaSimulada>>,
    fallar_creacion: Mutex<Option<String>>,
    fallar_enfoque: Mutex<Option<String>>,
}

impl EntornoSimulado {
    fn cerrar(&self, etiqueta: &str) {
        self.ventanas.lock().unwrap().remove(etiqueta);
    }

    fn total_abiertas(&self) -> usize {
        self.ventanas.lock().unwrap().len()
    }

    fn obtener(&self, etiqueta: &str) -> Option<VentanaSimulada> {
        self.ventanas.lock().unwrap().get(etiqueta).cloned()
    }
}

impl EntornoVentanas for EntornoSimulado {
    fn existe_ventana(&self, etiqueta: &str) -> bool {
        self.ventanas.lock().unwrap().contains_key(etiqueta)
    }

    fn crear_ventana(&self, config: &ConfiguracionVentana) -> Result<(), String> {
        if let Some(motivo) = self.fallar_creacion.lock().unwrap().clone() {
            return Err(motivo);
        }
        let mut mapa = self.ventanas.lock().unwrap();
        if mapa.contains_key(&config.etiqueta) {
            return Err(format!("Etiqueta duplicada: {}", config.etiqueta));
        }
        mapa.insert(
            config.etiqueta.clone(),
            VentanaSimulada {
                config: config.clone(),
                enfoques: 0,
            },
        );
        Ok(())
    }

    fn enfocar_ventana(&self, etiqueta: &str) -> Result<(), String> {
        if let Some(motivo) = self.fallar_enfoque.lock().unwrap().clone() {
            return Err(motivo);
        }
        let mut mapa = self.ventanas.lock().unwrap();
        let ventana = mapa
            .get_mut(etiqueta)
            .ok_or_else(|| format!("Ventana inexistente: {etiqueta}"))?;
        ventana.enfoques += 1;
        Ok(())
    }
}

#[test]
fn permite_multiples_ventanas_cliente_con_etiquetas_unicas_y_query_param() {
    let gestor = GestorVentanas::new();
    let entorno = EntornoSimulado::default();

    let primera = gestor.abrir(&entorno, TipoVista::Cliente).unwrap();
    let segunda = gestor.abrir(&entorno, TipoVista::Cliente).unwrap();
    let tercera = gestor.abrir(&entorno, TipoVista::Cliente).unwrap();

    assert_eq!(
        primera,
        ResultadoVentana::Creada {
            etiqueta: "cliente-1".to_string(),
            vista: TipoVista::Cliente,
        }
    );
    assert_eq!(
        segunda,
        ResultadoVentana::Creada {
            etiqueta: "cliente-2".to_string(),
            vista: TipoVista::Cliente,
        }
    );
    assert_eq!(
        tercera,
        ResultadoVentana::Creada {
            etiqueta: "cliente-3".to_string(),
            vista: TipoVista::Cliente,
        }
    );
    assert_eq!(entorno.total_abiertas(), 3);

    for etiqueta in ["cliente-1", "cliente-2", "cliente-3"] {
        let ventana = entorno.obtener(etiqueta).expect("debe existir ventana");
        assert_eq!(ventana.config.ruta_app, RUTA_VISTA_CLIENTE);
        assert!(ventana.config.etiqueta.starts_with(PREFIJO_VENTANA_CLIENTE));
    }
}

#[test]
fn crea_una_sola_ventana_cajero_y_enfoca_si_ya_existe_o_reabre_al_cerrarse() {
    let gestor = GestorVentanas::new();
    let entorno = EntornoSimulado::default();

    let creacion = gestor.abrir(&entorno, TipoVista::Cajero).unwrap();
    assert_eq!(
        creacion,
        ResultadoVentana::Creada {
            etiqueta: ETIQUETA_VENTANA_CAJERO.to_string(),
            vista: TipoVista::Cajero,
        }
    );
    assert_eq!(
        entorno
            .obtener(ETIQUETA_VENTANA_CAJERO)
            .unwrap()
            .config
            .ruta_app,
        RUTA_VISTA_CAJERO
    );

    let enfoque_1 = gestor.abrir(&entorno, TipoVista::Cajero).unwrap();
    let enfoque_2 = gestor.abrir(&entorno, TipoVista::Cajero).unwrap();
    assert_eq!(
        enfoque_1,
        ResultadoVentana::Enfocada {
            etiqueta: ETIQUETA_VENTANA_CAJERO.to_string(),
            vista: TipoVista::Cajero,
        }
    );
    assert_eq!(enfoque_1, enfoque_2);
    assert_eq!(entorno.total_abiertas(), 1);
    assert_eq!(
        entorno.obtener(ETIQUETA_VENTANA_CAJERO).unwrap().enfoques,
        2
    );

    entorno.cerrar(ETIQUETA_VENTANA_CAJERO);
    assert_eq!(entorno.total_abiertas(), 0);

    let recreacion = gestor.abrir(&entorno, TipoVista::Cajero).unwrap();
    assert_eq!(
        recreacion,
        ResultadoVentana::Creada {
            etiqueta: ETIQUETA_VENTANA_CAJERO.to_string(),
            vista: TipoVista::Cajero,
        }
    );
    assert_eq!(
        entorno.obtener(ETIQUETA_VENTANA_CAJERO).unwrap().enfoques,
        0
    );
}

#[test]
fn salta_etiquetas_cliente_preexistentes_y_soporta_concurrencia_sin_carrera() {
    let gestor = GestorVentanas::new();
    let entorno = EntornoSimulado::default();

    entorno
        .crear_ventana(&ConfiguracionVentana::para_vista(
            "cliente-1".to_string(),
            TipoVista::Cliente,
        ))
        .unwrap();

    let primera_libre = gestor.abrir(&entorno, TipoVista::Cliente).unwrap();
    assert_eq!(
        primera_libre,
        ResultadoVentana::Creada {
            etiqueta: "cliente-2".to_string(),
            vista: TipoVista::Cliente,
        }
    );

    let mut resultados = Vec::new();
    thread::scope(|scope| {
        let mut handles = Vec::new();
        for indice in 0..8 {
            let gestor_ref = &gestor;
            let entorno_ref = &entorno;
            handles.push(scope.spawn(move || {
                let vista = if indice % 2 == 0 {
                    TipoVista::Cajero
                } else {
                    TipoVista::Cliente
                };
                gestor_ref.abrir(entorno_ref, vista).unwrap()
            }));
        }
        for handle in handles {
            resultados.push(handle.join().unwrap());
        }
    });

    let cajeros_creados = resultados
        .iter()
        .filter(|r| {
            matches!(
                r,
                ResultadoVentana::Creada {
                    vista: TipoVista::Cajero,
                    ..
                }
            )
        })
        .count();
    let cajeros_enfocados = resultados
        .iter()
        .filter(|r| {
            matches!(
                r,
                ResultadoVentana::Enfocada {
                    vista: TipoVista::Cajero,
                    ..
                }
            )
        })
        .count();

    assert_eq!(cajeros_creados, 1);
    assert_eq!(cajeros_enfocados, 3);
    // 1 preexistente ("cliente-1") + 1 ("cliente-2") + 4 hilos cliente + 1 cajero = 7 ventanas.
    assert_eq!(entorno.total_abiertas(), 7);
}

#[test]
fn valida_entradas_de_tipo_vista_y_propaga_errores_del_entorno() {
    assert_eq!(
        parsear_tipo_vista(Some(json!("cliente"))).unwrap(),
        TipoVista::Cliente
    );
    assert_eq!(
        parsear_tipo_vista(Some(json!("cajero"))).unwrap(),
        TipoVista::Cajero
    );

    for invalida in [
        None,
        Some(json!(null)),
        Some(json!("")),
        Some(json!("admin")),
        Some(json!("CLIENTE")),
        Some(json!(42)),
    ] {
        let err = parsear_tipo_vista(invalida).unwrap_err();
        assert_eq!(err.codigo, CodigoError::EntradaInvalida);
        assert_eq!(err.recurso_id, None);
    }

    let gestor = GestorVentanas::new();
    let entorno = EntornoSimulado::default();
    *entorno.fallar_creacion.lock().unwrap() = Some("fallo simulado de WebView".to_string());

    let err_creacion = gestor.abrir(&entorno, TipoVista::Cajero).unwrap_err();
    assert_eq!(err_creacion.codigo, CodigoError::EntradaInvalida);
    assert_eq!(
        err_creacion.recurso_id.as_deref(),
        Some(ETIQUETA_VENTANA_CAJERO)
    );

    *entorno.fallar_creacion.lock().unwrap() = None;
    gestor.abrir(&entorno, TipoVista::Cajero).unwrap();
    *entorno.fallar_enfoque.lock().unwrap() = Some("ventana bloqueada".to_string());

    let err_enfoque = gestor.abrir(&entorno, TipoVista::Cajero).unwrap_err();
    assert_eq!(err_enfoque.codigo, CodigoError::EntradaInvalida);
    assert_eq!(
        err_enfoque.recurso_id.as_deref(),
        Some(ETIQUETA_VENTANA_CAJERO)
    );
}

#[test]
fn capabilities_default_incluye_ventanas_main_cajero_y_patron_cliente() {
    let contenido = include_str!("../capabilities/default.json");
    let config: serde_json::Value = serde_json::from_str(contenido).unwrap();
    let ventanas = config["windows"]
        .as_array()
        .expect("capabilities debe definir arreglo windows");

    assert!(ventanas.contains(&json!("main")));
    assert!(ventanas.contains(&json!(ETIQUETA_VENTANA_CAJERO)));
    assert!(ventanas.contains(&json!("cliente-*")));
}
