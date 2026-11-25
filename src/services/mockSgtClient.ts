import type { AppError, CodigoError, EstadoSistema, Turno, Ventanilla } from "../types/sgt";
import type { EscucharEstado, SgtClient } from "./sgtClient";

interface RegistroEscucha {
  callback: EscucharEstado;
}

interface Memoria {
  sistema: EstadoSistema;
  ultimaSecuencia: number;
}

function rechazar(codigo: CodigoError, mensaje: string, recursoId: string | null = null): never {
  throw { codigo, mensaje, recursoId } satisfies AppError;
}

function normalizarId(value: unknown, nombre: string): string {
  if (typeof value !== "string" || value.trim() === "") {
    rechazar("ENTRADA_INVALIDA", `${nombre} debe ser un identificador no vacío.`);
  }
  return value.trim();
}

function buscarVentanilla(memoria: Memoria, value: unknown): Ventanilla {
  const id = normalizarId(value, "ventanillaId");
  const ventanilla = memoria.sistema.ventanillas.find((item) => item.id === id);
  if (!ventanilla) rechazar("VENTANILLA_INVALIDA", `La ventanilla ${id} no existe.`, id);
  return ventanilla;
}

/**
 * Mock sin disco ni Tauri: el estado vive únicamente en esta instancia.
 * Compartir el mismo cliente entre consumidores permite simular sus eventos;
 * crear otro cliente o recargar pierde los datos. No sincroniza ventanas reales.
 */
export function createMockSgtClient(options: { now?: () => Date } = {}): SgtClient {
  const now = options.now ?? (() => new Date());
  let memoria: Memoria = {
    ultimaSecuencia: 0,
    sistema: {
      ventanillas: [
        { id: "V01", habilitada: true, turnoActualId: null },
        { id: "V02", habilitada: true, turnoActualId: null },
        { id: "V03", habilitada: true, turnoActualId: null },
      ],
      filaEspera: [],
      historial: [],
    },
  };
  // Registros separados permiten suscribir dos veces el mismo callback.
  const listeners = new Set<RegistroEscucha>();
  const pendientes: { snapshot: EstadoSistema; destinos: RegistroEscucha[] }[] = [];
  let emitiendo = false;

  function publicar(): void {
    pendientes.push({ snapshot: structuredClone(memoria.sistema), destinos: [...listeners] });
    if (emitiendo) return;
    emitiendo = true;
    try {
      // Cola FIFO: una mutación desde un listener no adelanta sus eventos
      // a los consumidores que aún no recibieron el snapshot anterior.
      while (pendientes.length > 0) {
        const evento = pendientes.shift()!;
        for (const destino of evento.destinos) {
          if (!listeners.has(destino)) continue;
          try {
            // No esperar al consumidor: el orden de publicación no depende de
            // su trabajo asíncrono, pero sus rechazos sí deben quedar aislados.
            void Promise.resolve(destino.callback(structuredClone(evento.snapshot))).catch(
              (error: unknown) => console.error("Error en un listener del mock SGT:", error),
            );
          } catch (error) {
            console.error("Error en un listener del mock SGT:", error);
          }
        }
      }
    } finally {
      emitiendo = false;
    }
  }

  function mutar<T>(operacion: (draft: Memoria) => T): T {
    const draft = structuredClone(memoria);
    const resultado = operacion(draft);
    // El historial solo se agrega, nunca se reordena: su secuencia es creciente.
    draft.sistema.filaEspera = draft.sistema.historial.filter((t) => t.estado === "EN_ESPERA");
    const copia = structuredClone(resultado);
    memoria = draft;
    publicar();
    return copia;
  }

  return {
    async obtener_estado() {
      return structuredClone(memoria.sistema);
    },
    async registrar_turno() {
      return mutar((draft) => {
        const secuencia = draft.ultimaSecuencia + 1;
        if (!Number.isSafeInteger(secuencia)) {
          rechazar("ENTRADA_INVALIDA", "Se agotó el rango de secuencias del mock.");
        }
        const fecha = now();
        if (!Number.isFinite(fecha.getTime())) {
          rechazar("ENTRADA_INVALIDA", "El reloj del mock devolvió una fecha inválida.");
        }
        const turno: Turno = {
          id: `T-${String(secuencia).padStart(4, "0")}`,
          secuencia,
          fechaHoraRegistro: fecha.toISOString(),
          estado: "EN_ESPERA",
          ventanillaAsignada: null,
        };
        draft.ultimaSecuencia = secuencia;
        draft.sistema.historial.push(turno);
        return turno;
      });
    },
    async asignar_siguiente(args) {
      return mutar((draft) => {
        const ventanilla = buscarVentanilla(draft, args?.ventanillaId);
        if (!ventanilla.habilitada) {
          rechazar(
            "VENTANILLA_INHABILITADA",
            `La ventanilla ${ventanilla.id} está inhabilitada.`,
            ventanilla.id,
          );
        }
        if (ventanilla.turnoActualId !== null) {
          rechazar(
            "VENTANILLA_OCUPADA",
            `La ventanilla ${ventanilla.id} está ocupada.`,
            ventanilla.id,
          );
        }
        const turno = draft.sistema.historial.find((t) => t.estado === "EN_ESPERA");
        if (!turno)
          rechazar(
            "FILA_VACIA",
            `No hay turnos para la ventanilla ${ventanilla.id}.`,
            ventanilla.id,
          );
        turno.estado = "EN_ATENCION";
        turno.ventanillaAsignada = ventanilla.id;
        ventanilla.turnoActualId = turno.id;
        return turno;
      });
    },
    async finalizar_atencion(args) {
      return mutar((draft) => {
        const ventanilla = buscarVentanilla(draft, args?.ventanillaId);
        const turno = draft.sistema.historial.find((t) => t.id === ventanilla.turnoActualId);
        if (!turno)
          rechazar(
            "VENTANILLA_LIBRE",
            `La ventanilla ${ventanilla.id} no tiene turno en atención.`,
            ventanilla.id,
          );
        turno.estado = "ATENDIDO";
        ventanilla.turnoActualId = null;
        return turno;
      });
    },
    async cancelar_turno(args) {
      return mutar((draft) => {
        const id = normalizarId(args?.turnoId, "turnoId");
        const turno = draft.sistema.historial.find((t) => t.id === id);
        if (!turno) rechazar("TURNO_INEXISTENTE", `El turno ${id} no existe.`, id);
        if (turno.estado !== "EN_ESPERA") {
          rechazar("ESTADO_TURNO_INVALIDO", `El turno ${id} no está en espera.`, id);
        }
        turno.estado = "CANCELADO";
        return turno;
      });
    },
    async cambiar_estado_ventanilla(args) {
      return mutar((draft) => {
        const ventanilla = buscarVentanilla(draft, args?.ventanillaId);
        if (typeof args.habilitada !== "boolean") {
          rechazar(
            "ENTRADA_INVALIDA",
            `habilitada debe ser booleano para ${ventanilla.id}.`,
            ventanilla.id,
          );
        }
        if (!args.habilitada && ventanilla.turnoActualId !== null) {
          rechazar(
            "VENTANILLA_OCUPADA",
            `La ventanilla ${ventanilla.id} está ocupada.`,
            ventanilla.id,
          );
        }
        if (
          !args.habilitada &&
          ventanilla.habilitada &&
          draft.sistema.ventanillas.filter((v) => v.habilitada).length === 1
        ) {
          rechazar(
            "ULTIMA_VENTANILLA_ACTIVA",
            `La ventanilla ${ventanilla.id} es la última habilitada.`,
            ventanilla.id,
          );
        }
        ventanilla.habilitada = args.habilitada;
        return ventanilla;
      });
    },
    async abrir_ventana(args) {
      if (args?.vista !== "cliente" && args?.vista !== "cajero") {
        rechazar("ENTRADA_INVALIDA", "vista debe ser cliente o cajero.");
      }
      // Simulación explícita: sin window.open ni WebviewWindowBuilder.
    },
    async escuchar_estado(callback) {
      if (typeof callback !== "function")
        rechazar("ENTRADA_INVALIDA", "El listener debe ser una función.");
      const registro = { callback };
      listeners.add(registro);
      return () => {
        listeners.delete(registro);
      };
    },
  };
}
