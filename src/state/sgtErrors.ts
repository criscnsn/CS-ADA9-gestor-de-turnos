import type { AppError, CodigoError } from "../types/sgt";

const CODIGOS_ERROR_CONOCIDOS: ReadonlySet<CodigoError> = new Set<CodigoError>([
  "VENTANILLA_INVALIDA",
  "VENTANILLA_OCUPADA",
  "VENTANILLA_LIBRE",
  "VENTANILLA_INHABILITADA",
  "ULTIMA_VENTANILLA_ACTIVA",
  "FILA_VACIA",
  "TURNO_INEXISTENTE",
  "ESTADO_TURNO_INVALIDO",
  "ENTRADA_INVALIDA",
  "ERROR_PERSISTENCIA",
  "BASE_DATOS_CORRUPTA",
]);

export type NotificacionSgt =
  | {
      tipo: "exito";
      mensaje: string;
      recursoId: string | null;
    }
  | {
      tipo: "error";
      codigo: CodigoError;
      mensaje: string;
      recursoId: string | null;
    };

export type ResultadoOperacion<T> = { ok: true; data: T } | { ok: false; error: AppError };

export function esCodigoError(valor: unknown): valor is CodigoError {
  return typeof valor === "string" && CODIGOS_ERROR_CONOCIDOS.has(valor as CodigoError);
}

/**
 * Verifica si un rechazo corresponde al objeto `AppError` definido en `docs/contrato_base.md`.
 * No depende de `instanceof Error` porque los comandos IPC y el mock rechazan con objetos planos.
 */
export function esAppError(error: unknown): error is AppError {
  if (typeof error !== "object" || error === null) {
    return false;
  }
  const candidato = error as Record<string, unknown>;
  return (
    esCodigoError(candidato.codigo) &&
    typeof candidato.mensaje === "string" &&
    candidato.mensaje.trim().length > 0 &&
    (candidato.recursoId === null || typeof candidato.recursoId === "string")
  );
}

/**
 * Convierte cualquier rechazo desconocido a un `AppError` utilizable por la interfaz
 * sin perder `codigo`, `mensaje` ni `recursoId` cuando el error ya cumple el contrato.
 */
export function normalizarError(error: unknown): AppError {
  if (esAppError(error)) {
    return {
      codigo: error.codigo,
      mensaje: error.mensaje,
      recursoId: error.recursoId,
    };
  }

  if (error instanceof Error && error.message.trim().length > 0) {
    return {
      codigo: "ERROR_PERSISTENCIA",
      mensaje: error.message,
      recursoId: null,
    };
  }

  return {
    codigo: "ERROR_PERSISTENCIA",
    mensaje: "Ocurrió un error inesperado al comunicarse con el sistema de turnos.",
    recursoId: null,
  };
}
