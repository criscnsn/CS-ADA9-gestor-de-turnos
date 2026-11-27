import type { TipoVista } from "../types/sgt";

export interface SelectorVistaProps {
  operando?: boolean;
  onSeleccionarVista: (vista: TipoVista) => void;
  onAbrirVentana?: (vista: TipoVista) => void | Promise<void>;
}

/**
 * Pantalla inicial para elegir Vista Cliente o Vista Cajero en la ventana actual
 * o solicitar la apertura de una ventana adicional mediante `abrir_ventana`.
 */
export function SelectorVista({
  operando = false,
  onSeleccionarVista,
  onAbrirVentana,
}: SelectorVistaProps) {
  return (
    <section className="sgt-selector" aria-labelledby="sgt-selector-titulo">
      <header className="sgt-selector__encabezado">
        <h1 id="sgt-selector-titulo">Sistema de Gestión de Turnos (SGT)</h1>
        <p>Selecciona la vista que deseas operar en esta ventana o abre una ventana adicional.</p>
      </header>

      <div className="sgt-selector__grid">
        <article className="sgt-selector__card">
          <h2>Vista Cliente</h2>
          <p>
            Sala de espera FIFO, estado de las ventanillas V01–V03 y solicitud o cancelación de
            turnos.
          </p>
          <div className="sgt-selector__acciones">
            <button
              type="button"
              className="sgt-btn sgt-btn--primario"
              onClick={() => onSeleccionarVista("cliente")}
            >
              Entrar como Cliente
            </button>
            {onAbrirVentana && (
              <button
                type="button"
                className="sgt-btn sgt-btn--secundario"
                disabled={operando}
                onClick={() => {
                  void onAbrirVentana("cliente");
                }}
              >
                Abrir ventana Cliente
              </button>
            )}
          </div>
        </article>

        <article className="sgt-selector__card">
          <h2>Vista Cajero</h2>
          <p>
            Operación de ventanillas V01–V03, asignación FIFO, finalización, cancelación por ID e
            historial.
          </p>
          <div className="sgt-selector__acciones">
            <button
              type="button"
              className="sgt-btn sgt-btn--primario"
              onClick={() => onSeleccionarVista("cajero")}
            >
              Entrar como Cajero
            </button>
            {onAbrirVentana && (
              <button
                type="button"
                className="sgt-btn sgt-btn--secundario"
                disabled={operando}
                onClick={() => {
                  void onAbrirVentana("cajero");
                }}
              >
                Abrir ventana Cajero
              </button>
            )}
          </div>
        </article>
      </div>
    </section>
  );
}
