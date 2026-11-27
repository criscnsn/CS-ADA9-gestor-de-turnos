import { useEffect, useId } from "react";

export interface ConfirmacionModalProps {
  abierto: boolean;
  titulo: string;
  mensaje: string;
  textoConfirmar?: string;
  textoCancelar?: string;
  procesando?: boolean;
  onConfirmar: () => void | Promise<void>;
  onCancelar: () => void;
}

/**
 * Modal accesible para confirmaciones explícitas antes de cancelar un turno (`RNF-U04`).
 * Rechazar o cerrar mediante `Escape` nunca dispara `onConfirmar`.
 */
export function ConfirmacionModal({
  abierto,
  titulo,
  mensaje,
  textoConfirmar = "Confirmar",
  textoCancelar = "Volver",
  procesando = false,
  onConfirmar,
  onCancelar,
}: ConfirmacionModalProps) {
  const tituloId = useId();
  const descripcionId = useId();

  useEffect(() => {
    if (!abierto) return;
    const manejarTeclado = (evento: KeyboardEvent) => {
      if (evento.key === "Escape" && !procesando) {
        onCancelar();
      }
    };
    window.addEventListener("keydown", manejarTeclado);
    return () => {
      window.removeEventListener("keydown", manejarTeclado);
    };
  }, [abierto, procesando, onCancelar]);

  if (!abierto) {
    return null;
  }

  return (
    <div
      className="sgt-modal-backdrop"
      role="presentation"
      onClick={(evento) => {
        if (evento.target === evento.currentTarget && !procesando) {
          onCancelar();
        }
      }}
    >
      <div
        className="sgt-modal"
        role="dialog"
        aria-modal="true"
        aria-labelledby={tituloId}
        aria-describedby={descripcionId}
      >
        <h2 id={tituloId} className="sgt-modal__titulo">
          {titulo}
        </h2>
        <p id={descripcionId} className="sgt-modal__mensaje">
          {mensaje}
        </p>
        <div className="sgt-modal__acciones">
          <button
            type="button"
            className="sgt-btn sgt-btn--secundario"
            onClick={onCancelar}
            disabled={procesando}
          >
            {textoCancelar}
          </button>
          <button
            type="button"
            className="sgt-btn sgt-btn--peligro"
            onClick={() => {
              void onConfirmar();
            }}
            disabled={procesando}
          >
            {procesando ? "Procesando..." : textoConfirmar}
          </button>
        </div>
      </div>
    </div>
  );
}
