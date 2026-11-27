import type { EstadoTurno, Ventanilla } from "../types/sgt";

export type EstadoVentanilla = "LIBRE" | "OCUPADO" | "INHABILITADO";

export interface EtiquetaEstado {
  texto: string;
  simbolo: string;
  variante: string;
}

export const ETIQUETAS_TURNO: Record<EstadoTurno, EtiquetaEstado> = {
  EN_ESPERA: { texto: "En espera", simbolo: "●", variante: "espera" },
  EN_ATENCION: { texto: "En atención", simbolo: "▶", variante: "atencion" },
  ATENDIDO: { texto: "Atendido", simbolo: "✓", variante: "atendido" },
  CANCELADO: { texto: "Cancelado", simbolo: "✕", variante: "cancelado" },
};

export const ETIQUETAS_VENTANILLA: Record<EstadoVentanilla, EtiquetaEstado> = {
  LIBRE: { texto: "Libre", simbolo: "○", variante: "libre" },
  OCUPADO: { texto: "Ocupado", simbolo: "◆", variante: "ocupado" },
  INHABILITADO: { texto: "Inhabilitado", simbolo: "⊘", variante: "inhabilitado" },
};

/**
 * Deriva el estado visible de una ventanilla a partir de `habilitada` y `turnoActualId`
 * conforme a `docs/contrato_base.md` y `RF-13`.
 */
export function derivarEstadoVentanilla(ventanilla: Ventanilla): EstadoVentanilla {
  if (!ventanilla.habilitada) {
    return "INHABILITADO";
  }
  return ventanilla.turnoActualId !== null ? "OCUPADO" : "LIBRE";
}
