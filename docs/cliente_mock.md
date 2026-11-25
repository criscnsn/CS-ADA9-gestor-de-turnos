# Cliente de turnos para desarrollo y pruebas

`SgtClient` en `src/services/sgtClient.ts` define el puerto asíncrono del frontend. `createMockSgtClient` implementa sus siete comandos y la suscripción al estado sin cargar Tauri ni usar SQL.

```ts
import { createMockSgtClient } from "../services/mockSgtClient";

const client = createMockSgtClient();
// Inyectar esta misma instancia en las pantallas que compartirán estado.
const desuscribir = await client.escuchar_estado((estado) => {
  console.log(estado.filaEspera);
});
const inicial = await client.obtener_estado();
const turno = await client.registrar_turno();
await client.cancelar_turno({ turnoId: turno.id });
// Al desmontar el consumidor; repetir la llamada es seguro.
desuscribir();
```

Las operaciones reciben objetos de parámetros con campos camelCase, tal como el [contrato](./contrato_base.md). Todos sus retornos son Promises y los rechazos funcionales tienen la forma de `AppError`: `codigo`, `mensaje` en español y `recursoId` cuando aplica. La UI debe manejar el rechazo; no depender de `instanceof Error`.

El cliente empieza con V01, V02 y V03 habilitadas y sin turnos. Cada registro exitoso genera una secuencia creciente y fecha ISO-8601. Para pruebas deterministas puede usarse `createMockSgtClient({ now: () => new Date("2026-11-18T01:00:00.000Z") })`.

## Eventos y aislamiento

La escucha no envía un estado inicial: obtenerlo con `obtener_estado`. Cada mutación exitosa publica exactamente un snapshot equivalente al payload de `sgt://estado-actualizado`; una operación rechazada no modifica estado/contador ni publica eventos. Repetir una petición de habilitación ya aplicada es un éxito y también publica el snapshot. Las consultas y `abrir_ventana` no emiten cambios de dominio.

Cada consumidor recibe copias independientes. Un listener que lanza un error o cuya Promise rechaza se reporta por consola sin rechazar una mutación ya aplicada ni impedir notificaciones a otros consumidores. La cola de publicación conserva el orden si un listener provoca otra mutación. Suscribir dos veces el mismo callback crea dos suscripciones independientes.

## Límites

- Solo se comparte estado entre consumidores de la misma instancia. Crear otro cliente, recargar la página o reiniciar pierde los datos.
- `abrir_ventana` valida `cliente`/`cajero` y resuelve sin crear ventanas reales.
- Los tipos incluyen los errores de persistencia, pero el mock no certifica transacciones SQLite, recuperación, rendimiento ni sincronización multi-ventana.
- La integración real de `invoke`/`listen` y el manejo de carreras entre hidratación y eventos pertenecen al adaptador Tauri futuro.

## Verificación

Ejecutar `pnpm test` y `pnpm build`. Las pruebas TS leen los mismos [fixtures JSON](./fixtures/contrato/README.md) que Rust. Como los imports JSON ensanchan strings y arrays, los tests TS validan los campos y construyen valores de los tipos del contrato, sin casts que omitan esa comprobación. También se prueban FIFO, rechazos, RF-13, copias, eventos y reloj inyectado.
