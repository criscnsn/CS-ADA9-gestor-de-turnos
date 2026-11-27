import type { AppError } from "../types/sgt";

export interface ErrorBaseDatosViewProps {
  error: AppError;
  reintentando?: boolean;
  onReintentar?: () => void | Promise<void>;
}

/**
 * Aviso visible para `BASE_DATOS_CORRUPTA` (`RD-03`).
 * Informa claramente la falla y garantiza que no se sobrescriben datos en silencio.
 */
export function ErrorBaseDatosView({
  error,
  reintentando = false,
  onReintentar,
}: ErrorBaseDatosViewProps) {
  return (
    <section className="sgt-error-critico" role="alert" aria-live="assertive">
      <div className="sgt-error-critico__card">
        <p className="sgt-error-critico__etiqueta">
          Error crítico de persistencia ({error.codigo})
        </p>
        <h1 className="sgt-error-critico__titulo">No se pudo leer la base de datos local</h1>
        <p className="sgt-error-critico__mensaje">{error.mensaje}</p>
        {error.recursoId && (
          <p className="sgt-error-critico__recurso">Recurso implicado: {error.recursoId}</p>
        )}
        <p className="sgt-error-critico__aviso">
          Por seguridad, la aplicación no sobrescribió ni eliminó los datos existentes. Verifica o
          respalda el archivo local antes de reintentar.
        </p>
        {onReintentar && (
          <button
            type="button"
            className="sgt-btn sgt-btn--primario"
            onClick={() => {
              void onReintentar();
            }}
            disabled={reintentando}
          >
            {reintentando ? "Verificando..." : "Reintentar carga"}
          </button>
        )}
      </div>
    </section>
  );
}
