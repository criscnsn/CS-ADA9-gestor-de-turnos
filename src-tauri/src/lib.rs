//! Backend nativo del Sistema de Gestión de Turnos (SGT).
//!
//! [`models`] expone los tipos JSON compartidos con el frontend. [`run`] inicia
//! la aplicación Tauri con sus plugins y comandos registrados.
//!
//! Los modelos describen el contrato de datos, [`domain`] encapsula el estado y
//! las reglas puras del SGT y [`db`] inicializa el esquema SQLite. Los módulos
//! de transiciones, `RF-13` y comandos IPC se incorporan en sus respectivos
//! entregables.

#![warn(missing_docs, rustdoc::broken_intra_doc_links)]

pub mod db;
pub mod domain;
pub mod models;

// Learn more about Tauri commands at https://tauri.app/develop/calling-rust/
#[tauri::command]
fn greet(name: &str) -> String {
    format!("Hello, {}! You've been greeted from Rust!", name)
}

/// Inicia la aplicación Tauri y ejecuta su ciclo de eventos.
///
/// Registra el plugin de apertura y los comandos disponibles en este punto de
/// entrada. La función se utiliza desde el binario de escritorio y, cuando se
/// compila para dispositivos móviles, como punto de entrada nativo.
///
/// # Panics
///
/// Entra en pánico si Tauri devuelve un error al inicializar o ejecutar la
/// aplicación. Esta función no debe invocarse como parte de un ejemplo de datos.
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![greet])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
