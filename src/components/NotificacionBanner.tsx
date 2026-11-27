import type { NotificacionSgt } from "../state/sgtErrors";

export interface NotificacionBannerProps {
  notificacion: NotificacionSgt | null;
  onCerrar?: () => void;
}

/**
 * Banner de retroalimentación para confirmaciones y errores operativos (`RF-12`, `RNF-U02`).
 * Muestra prefijo textual, causa e identificador implicado cuando corresponde.
 */
export function NotificacionBanner({ notificacion, onCerrar }: NotificacionBannerProps) {
  if (!notificacion) {
    return null;
  }

  const esError = notificacion.tipo === "error";

  return (
    <div
      className={`sgt-banner ${esError ? "sgt-banner--error" : "sgt-banner--exito"}`}
      role={esError ? "alert" : "status"}
      aria-live={esError ? "assertive" : "polite"}
    >
      <div className="sgt-banner__contenido">
        <strong className="sgt-banner__prefijo">{esError ? "Error" : "Confirmación"}:</strong>{" "}
        <span className="sgt-banner__mensaje">{notificacion.mensaje}</span>
        {notificacion.recursoId && (
          <span className="sgt-banner__recurso" data-testid="notificacion-recurso-id">
            ID: {notificacion.recursoId}
          </span>
        )}
        {esError && (
          <span className="sgt-banner__codigo" data-testid="notificacion-codigo">
            ({notificacion.codigo})
          </span>
        )}
      </div>
      {onCerrar && (
        <button
          type="button"
          className="sgt-btn sgt-btn--texto"
          onClick={onCerrar}
          aria-label="Cerrar notificación"
        >
          Cerrar
        </button>
      )}
    </div>
  );
}
