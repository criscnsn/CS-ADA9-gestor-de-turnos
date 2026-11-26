import type { AppError, EstadoSistema, TipoVista } from "../types/sgt";
import type { Desuscribir, EscucharEstado, SgtClient } from "./sgtClient";

/** Resultado de hidratar y suscribir una ventana activa al flujo de estado SGT. */
export interface ConexionVentanaSgt {
  /** Snapshot vigente obtenido al completar la hidratación sin condiciones de carrera. */
  estadoInicial: EstadoSistema;
  /** Cancela la suscripción de eventos de esta ventana; es idempotente. */
  cerrar: Desuscribir;
}

/**
 * Extrae y valida la vista (`"cliente"` o `"cajero"`) desde un query string o ruta relativa.
 *
 * Acepta tanto `"?vista=cliente"` como rutas estilo `"index.html?vista=cajero"`
 * generadas por `WebviewWindowBuilder` en el comando `abrir_ventana`.
 * Devuelve `null` si el parámetro no existe, está repetido o contiene un valor no admitido.
 */

export function resolverVistaDesdeUrl(searchORuta: string): TipoVista | null {
  if (typeof searchORuta !== "string" || searchORuta.trim() === "") {
    return null;
  }

  const indiceQuery = searchORuta.indexOf("?");
  const queryLimpia = indiceQuery >= 0 ? searchORuta.slice(indiceQuery + 1) : searchORuta;
  const params = new URLSearchParams(queryLimpia);
  const valores = params.getAll("vista");

  if (valores.length !== 1) {
    return null;
  }

  const vista = valores[0];
  if (vista === "cliente" || vista === "cajero") {
    return vista;
  }

  return null;
}

/**
 * Hidrata una ventana con `obtener_estado()` y la mantiene sincronizada mediante
 * `escuchar_estado()` (`"sgt://estado-actualizado"`) sin condiciones de carrera
 * ni estado persistente duplicado por ventana.
 *
 * Orden de seguridad:
 * 1. Se registra primero el listener de eventos antes de solicitar `obtener_estado()`.
 * 2. Si llega un evento `"sgt://estado-actualizado"` mientras `obtener_estado()` sigue
 *    en vuelo, se retiene el snapshot más reciente del evento para evitar que la
 *    respuesta tardía de `obtener_estado()` sobrescriba la ventana con estado obsoleto.
 * 3. Si `obtener_estado()` falla (por ejemplo, `BASE_DATOS_CORRUPTA`), se limpia
 *    automáticamente la suscripción antes de propagar el rechazo.
 */
export async function hidratarYSincronizarVentana(
  client: SgtClient,
  onEstado: EscucharEstado,
): Promise<ConexionVentanaSgt> {
  if (typeof onEstado !== "function") {
    const error: AppError = {
      codigo: "ENTRADA_INVALIDA",
      mensaje: "onEstado debe ser una función.",
      recursoId: null,
    };
    throw error;
  }

  let cerrada = false;
  let hidratando = true;
  let ultimoEventoEnVuelo: EstadoSistema | null = null;

  const desuscribir = await client.escuchar_estado((nuevoEstado) => {
    if (cerrada) return;
    if (hidratando) {
      ultimoEventoEnVuelo = structuredClone(nuevoEstado);
      return;
    }
    onEstado(structuredClone(nuevoEstado));
  });

  const cerrar: Desuscribir = () => {
    if (cerrada) return;
    cerrada = true;
    ultimoEventoEnVuelo = null;
    desuscribir();
  };

  try {
    const estadoRemoto = await client.obtener_estado();
    const vigente = structuredClone(ultimoEventoEnVuelo ?? estadoRemoto);
    ultimoEventoEnVuelo = null;
    hidratando = false;

    if (!cerrada) {
      onEstado(structuredClone(vigente));
    }

    return {
      estadoInicial: structuredClone(vigente),
      cerrar,
    };
  } catch (error) {
    cerrar();
    throw error;
  }
}
