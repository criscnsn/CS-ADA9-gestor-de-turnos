import type { EstadoTurno, Ventanilla } from "../types/sgt";
import { ETIQUETAS_TURNO, ETIQUETAS_VENTANILLA, derivarEstadoVentanilla } from "./estadoLabels";
import type { EstadoVentanilla } from "./estadoLabels";

export type EstadoBadgeProps =
  | {
      tipo: "turno";
      estado: EstadoTurno;
      className?: string;
    }
  | {
      tipo: "ventanilla";
      estado: EstadoVentanilla;
      className?: string;
    }
  | {
      tipo: "ventanilla";
      ventanilla: Ventanilla;
      className?: string;
    };

/**
 * Insignia accesible para estados de turno y ventanilla (`RNF-U03`).
 * Combina color, borde, símbolo textual y etiqueta explícita en español.
 */
export function EstadoBadge(props: EstadoBadgeProps) {
  const clave =
    props.tipo === "turno"
      ? props.estado
      : "ventanilla" in props
        ? derivarEstadoVentanilla(props.ventanilla)
        : props.estado;

  const info =
    props.tipo === "turno"
      ? ETIQUETAS_TURNO[props.estado]
      : ETIQUETAS_VENTANILLA[clave as EstadoVentanilla];

  const clases = ["sgt-badge", `sgt-badge--${info.variante}`, props.className]
    .filter(Boolean)
    .join(" ");

  return (
    <span
      className={clases}
      data-tipo={props.tipo}
      data-estado={clave}
      aria-label={`Estado: ${info.texto}`}
    >
      <span className="sgt-badge__simbolo" aria-hidden="true">
        {info.simbolo}
      </span>
      <span className="sgt-badge__texto">{info.texto}</span>
    </span>
  );
}
