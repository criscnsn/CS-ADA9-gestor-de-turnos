//! Backend nativo del Sistema de Gestión de Turnos (SGT).
//!
//! [`models`] expone los tipos JSON compartidos con el frontend. [`run`] inicia
//! la aplicación Tauri con sus plugins, estado gestionado y comandos registrados.
//!
//! Los modelos describen el contrato de datos, [`domain`] encapsula el estado y
//! las reglas puras del SGT, [`db`] y [`repository`] gestionan la persistencia
//! transaccional en SQLite, [`windows`] coordina las ventanas Cliente/Cajero y
//! [`comandos`] expone los comandos IPC con el patrón `commit → memoria → evento`.

#![warn(missing_docs, rustdoc::broken_intra_doc_links)]

use tauri::{Manager, Runtime};

pub mod comandos;
pub mod db;
pub mod domain;
pub mod models;
pub mod repository;
pub mod windows;

// Learn more about Tauri commands at https://tauri.app/develop/calling-rust/
#[tauri::command]
fn greet(name: &str) -> String {
    format!("Hello, {}! You've been greeted from Rust!", name)
}

/// Registra en `builder` el despachador IPC completo de la aplicación.
///
/// Se expone genérico sobre [`Runtime`] para que las pruebas de integración
/// puedan ejercitar exactamente los mismos comandos con `MockRuntime` y una
/// base de datos temporal aislada.
#[must_use]
pub fn configurar_comandos<R: Runtime>(builder: tauri::Builder<R>) -> tauri::Builder<R> {
    builder.invoke_handler(tauri::generate_handler![
        greet,
        windows::abrir_ventana,
        comandos::obtener_estado,
        comandos::registrar_turno,
        comandos::asignar_siguiente,
        comandos::finalizar_atencion,
        comandos::cancelar_turno,
        comandos::cambiar_estado_ventanilla,
    ])
}

/// Inicia la aplicación Tauri y ejecuta su ciclo de eventos.
///
/// Resuelve `app_data_dir()/sgt.db` durante `setup`, inicializa [`comandos::EstadoApp`]
/// mediante [`comandos::inicializar_estado_app`] (conservando errores de arranque
/// como `BASE_DATOS_CORRUPTA` sin sobrescribir archivos en disco) y registra los
/// comandos disponibles en este punto de entrada.
///
/// # Panics
///
/// Entra en pánico si Tauri devuelve un error al inicializar o ejecutar la
/// aplicación. Esta función no debe invocarse como parte de un ejemplo de datos.
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    configurar_comandos(
        tauri::Builder::default()
            .plugin(tauri_plugin_opener::init())
            .setup(|app| {
                let estado = comandos::inicializar_estado_app(app.handle());
                app.manage(estado);
                Ok(())
            }),
    )
    .run(tauri::generate_context!())
    .expect("error while running tauri application");
}
