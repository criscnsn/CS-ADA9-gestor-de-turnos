# Fixtures compartidos del contrato SGT

Estos ejemplos verifican el mismo formato JSON en Rust y TypeScript. Su fuente es [contrato_base.md](../../contrato_base.md).

- `estado-vacio.json`: tres ventanillas habilitadas, sin turnos; incluye null explícitos.
- `estado-mixto.json`: los cuatro estados de turno, una atención activa, historial y fila FIFO.
- `errores.json`: los once códigos de error con `mensaje` y `recursoId` (incluye null).
- `enumeraciones.json`: todos los valores exactos de estados, ventanillas, vistas y errores.

Las pruebas Rust de `src-tauri/tests/contrato_json.rs` deserializan y vuelven a serializar cada ejemplo comparando valores JSON, sin depender del orden de las claves. Los DTO no validan las reglas del dominio: una deserialización exitosa no demuestra consistencia de negocio.

Los consumidores TS también deben verificar estos mismos archivos. Cambiar un fixture o el contrato exige validar ambos lenguajes antes de integrar; no mantener versiones divergentes por lenguaje.
