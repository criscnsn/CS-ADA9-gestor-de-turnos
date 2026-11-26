//! Gestión de ventanas nativas para Vista Cliente y Vista Cajero (`#07-A`).
//!
//! Este módulo encapsula la política multi-ventana del contrato base:
//! - [`TipoVista::Cajero`] utiliza una única etiqueta fija ([`ETIQUETA_VENTANA_CAJERO`]);
//!   si la ventana ya está abierta, se restaura y enfoca en lugar de duplicarse.
//! - [`TipoVista::Cliente`] genera etiquetas únicas incrementales con el prefijo
//!   [`PREFIJO_VENTANA_CLIENTE`] (`cliente-1`, `cliente-2`, ...), permitiendo
//!   múltiples instancias simultáneas sobre el mismo proceso local.
//!
//! La sincronización de datos (`obtener_estado` y `sgt://estado-actualizado`) no
//! mantiene estado por ventana aquí: todas las vistas consumen el estado único
//! del backend expuesto por los comandos IPC del sistema.

use std::path::PathBuf;
use std::sync::Mutex;

use serde_json::Value;
use tauri::{AppHandle, Manager, Runtime, WebviewUrl, WebviewWindowBuilder};

use crate::models::{AppError, CodigoError, TipoVista};

/// Etiqueta única reservada para la ventana operativa del cajero.
pub const ETIQUETA_VENTANA_CAJERO: &str = "cajero";

/// Prefijo utilizado para numerar cada ventana independiente de cliente.
pub const PREFIJO_VENTANA_CLIENTE: &str = "cliente-";

/// Ruta relativa con query parameter cargada por las ventanas de cliente.
pub const RUTA_VISTA_CLIENTE: &str = "index.html?vista=cliente";

/// Ruta relativa con query parameter cargada por la ventana de cajero.
pub const RUTA_VISTA_CAJERO: &str = "index.html?vista=cajero";

static GESTOR_VENTANAS_GLOBAL: GestorVentanas = GestorVentanas::new();

/// Parámetros de construcción para una nueva ventana `WebviewWindow`.
#[derive(Debug, Clone, PartialEq)]
pub struct ConfiguracionVentana {
    /// Etiqueta única de la ventana dentro del proceso Tauri.
    pub etiqueta: String,
    /// Tipo de vista solicitada según el contrato compartido.
    pub vista: TipoVista,
    /// Ruta interna de la aplicación incluyendo `?vista=cliente` o `?vista=cajero`.
    pub ruta_app: String,
    /// Título visible en la barra de la ventana del sistema operativo.
    pub titulo: String,
    /// Ancho inicial en píxeles lógicos.
    pub ancho: f64,
    /// Alto inicial en píxeles lógicos.
    pub alto: f64,
    /// Ancho mínimo permitido en píxeles lógicos.
    pub ancho_minimo: f64,
    /// Alto mínimo permitido en píxeles lógicos.
    pub alto_minimo: f64,
}

impl ConfiguracionVentana {
    /// Construye la configuración estándar asociada a una etiqueta y [`TipoVista`].
    #[must_use]
    pub fn para_vista(etiqueta: String, vista: TipoVista) -> Self {
        match vista {
            TipoVista::Cliente => Self {
                etiqueta,
                vista,
                ruta_app: RUTA_VISTA_CLIENTE.to_string(),
                titulo: "SGT — Vista Cliente".to_string(),
                ancho: 920.0,
                alto: 680.0,
                ancho_minimo: 640.0,
                alto_minimo: 480.0,
            },
            TipoVista::Cajero => Self {
                etiqueta,
                vista,
                ruta_app: RUTA_VISTA_CAJERO.to_string(),
                titulo: "SGT — Vista Cajero".to_string(),
                ancho: 1120.0,
                alto: 740.0,
                ancho_minimo: 800.0,
                alto_minimo: 600.0,
            },
        }
    }
}

/// Acción efectiva ejecutada por [`GestorVentanas::abrir`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResultadoVentana {
    /// Se construyó una ventana nueva con la etiqueta indicada.
    Creada {
        /// Etiqueta asignada a la nueva ventana.
        etiqueta: String,
        /// Vista cargada en la ventana creada.
        vista: TipoVista,
    },
    /// La ventana ya existía, por lo que fue restaurada y enfocada.
    Enfocada {
        /// Etiqueta existente que recibió el foco.
        etiqueta: String,
        /// Vista asociada a la ventana enfocada.
        vista: TipoVista,
    },
}

/// Abstracción del runtime de ventanas para desacoplar pruebas unitarias de la GUI nativa.
pub trait EntornoVentanas {
    /// Comprueba si actualmente existe una ventana abierta con `etiqueta`.
    fn existe_ventana(&self, etiqueta: &str) -> bool;

    /// Crea una nueva ventana nativa a partir de `config`.
    ///
    /// # Errors
    ///
    /// Devuelve una descripción textual si el runtime no logra construir la ventana.
    fn crear_ventana(&self, config: &ConfiguracionVentana) -> Result<(), String>;

    /// Desminimiza, muestra y enfoca una ventana previamente creada.
    ///
    /// # Errors
    ///
    /// Devuelve una descripción textual si la ventana ya no existe o falla el enfoque.
    fn enfocar_ventana(&self, etiqueta: &str) -> Result<(), String>;
}

impl<R: Runtime> EntornoVentanas for AppHandle<R> {
    fn existe_ventana(&self, etiqueta: &str) -> bool {
        self.get_webview_window(etiqueta).is_some()
    }

    fn crear_ventana(&self, config: &ConfiguracionVentana) -> Result<(), String> {
        WebviewWindowBuilder::new(
            self,
            &config.etiqueta,
            WebviewUrl::App(PathBuf::from(&config.ruta_app)),
        )
        .title(&config.titulo)
        .inner_size(config.ancho, config.alto)
        .min_inner_size(config.ancho_minimo, config.alto_minimo)
        .build()
        .map(|_| ())
        .map_err(|err| err.to_string())
    }

    fn enfocar_ventana(&self, etiqueta: &str) -> Result<(), String> {
        let ventana = self
            .get_webview_window(etiqueta)
            .ok_or_else(|| format!("La ventana '{etiqueta}' ya no se encuentra abierta."))?;
        if ventana.is_minimized().unwrap_or(false) {
            ventana.unminimize().map_err(|err| err.to_string())?;
        }
        ventana.show().map_err(|err| err.to_string())?;
        ventana.set_focus().map_err(|err| err.to_string())
    }
}

/// Coordinador concurrente de etiquetas y ciclo de apertura/enfoque de ventanas.
///
/// Serializa las operaciones de apertura mediante un [`Mutex`] interno para evitar
/// condiciones de carrera cuando se solicitan múltiples aperturas simultáneas
/// (por ejemplo, doble clic rápido al abrir la ventana única de cajero).
#[derive(Debug)]
pub struct GestorVentanas {
    ultimo_cliente_id: Mutex<u64>,
}

impl Default for GestorVentanas {
    fn default() -> Self {
        Self::new()
    }
}

impl GestorVentanas {
    /// Crea un coordinador con contador inicializado en cero.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            ultimo_cliente_id: Mutex::new(0),
        }
    }

    /// Ejecuta la política de creación o enfoque según la [`TipoVista`] indicada.
    ///
    /// # Errors
    ///
    /// Devuelve [`AppError`] si se agota el contador de etiquetas o si el entorno
    /// nativo rechaza crear o enfocar la ventana solicitada.
    pub fn abrir<E: EntornoVentanas>(
        &self,
        entorno: &E,
        vista: TipoVista,
    ) -> Result<ResultadoVentana, AppError> {
        let mut ultimo_id = self
            .ultimo_cliente_id
            .lock()
            .unwrap_or_else(|envenenado| envenenado.into_inner());

        match vista {
            TipoVista::Cajero => {
                let etiqueta = ETIQUETA_VENTANA_CAJERO.to_string();
                if entorno.existe_ventana(&etiqueta) {
                    entorno.enfocar_ventana(&etiqueta).map_err(|detalle| {
                        error_operacion_ventana("enfocar", &etiqueta, &detalle)
                    })?;
                    return Ok(ResultadoVentana::Enfocada { etiqueta, vista });
                }

                let config = ConfiguracionVentana::para_vista(etiqueta.clone(), vista);
                entorno
                    .crear_ventana(&config)
                    .map_err(|detalle| error_operacion_ventana("abrir", &etiqueta, &detalle))?;
                Ok(ResultadoVentana::Creada { etiqueta, vista })
            }
            TipoVista::Cliente => {
                let etiqueta = reservar_etiqueta_cliente(entorno, &mut ultimo_id)?;
                let config = ConfiguracionVentana::para_vista(etiqueta.clone(), vista);
                entorno
                    .crear_ventana(&config)
                    .map_err(|detalle| error_operacion_ventana("abrir", &etiqueta, &detalle))?;
                Ok(ResultadoVentana::Creada { etiqueta, vista })
            }
        }
    }
}

fn reservar_etiqueta_cliente<E: EntornoVentanas>(
    entorno: &E,
    ultimo_id: &mut u64,
) -> Result<String, AppError> {
    let mut candidato = *ultimo_id;
    loop {
        candidato = candidato.checked_add(1).ok_or_else(|| AppError {
            codigo: CodigoError::EntradaInvalida,
            mensaje: "Se agotó el rango de identificadores para ventanas de cliente.".to_string(),
            recurso_id: None,
        })?;
        let etiqueta = format!("{PREFIJO_VENTANA_CLIENTE}{candidato}");
        if !entorno.existe_ventana(&etiqueta) {
            *ultimo_id = candidato;
            return Ok(etiqueta);
        }
    }
}

fn error_operacion_ventana(accion: &str, etiqueta: &str, detalle: &str) -> AppError {
    AppError {
        codigo: CodigoError::EntradaInvalida,
        mensaje: format!("No se pudo {accion} la ventana '{etiqueta}': {detalle}"),
        recurso_id: Some(etiqueta.to_string()),
    }
}

/// Valida y convierte el argumento crudo de IPC en [`TipoVista`].
///
/// Devuelve [`CodigoError::EntradaInvalida`] con `recurso_id: None` cuando el
/// argumento está ausente o no coincide con `"cliente"` ni `"cajero"`, manteniendo
/// paridad exacta con el contrato JSON y el cliente mock.
///
/// # Errors
///
/// Retorna [`AppError`] con [`CodigoError::EntradaInvalida`] si `vista` es [`None`]
/// o contiene un valor distinto de `"cliente"` y `"cajero"`.
pub fn parsear_tipo_vista(vista: Option<Value>) -> Result<TipoVista, AppError> {
    let valor = vista.ok_or_else(error_vista_invalida)?;
    serde_json::from_value::<TipoVista>(valor).map_err(|_| error_vista_invalida())
}

fn error_vista_invalida() -> AppError {
    AppError {
        codigo: CodigoError::EntradaInvalida,
        mensaje: "vista debe ser cliente o cajero.".to_string(),
        recurso_id: None,
    }
}

/// Comando IPC de Tauri que crea o enfoca una ventana `cliente` o `cajero`.
///
/// Se declara `async` para ejecutarse fuera del hilo principal de la interfaz y
/// evitar bloqueos al construir instancias `WebView2` en Windows.
///
/// # Errors
///
/// Devuelve [`AppError`] si `vista` no es válida o si el sistema de ventanas
/// rechaza construir o enfocar la ventana solicitada.
#[tauri::command]
pub async fn abrir_ventana<R: Runtime>(
    app: AppHandle<R>,
    vista: Option<Value>,
) -> Result<(), AppError> {
    let tipo_vista = parsear_tipo_vista(vista)?;
    GESTOR_VENTANAS_GLOBAL.abrir(&app, tipo_vista).map(|_| ())
}
