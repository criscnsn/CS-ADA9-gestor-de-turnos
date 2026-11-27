import { EstadoBadge } from "../components/EstadoBadge";
import { useSgt } from "../state/sgtContext";

/**
 * Contenedor base para Vista Cajero (la interfaz completa se implementa en `#09`).
 */
export function VentanillaPage() {
  const { estado } = useSgt();

  return (
    <section className="sgt-placeholder" aria-labelledby="vista-cajero-titulo">
      <h1 id="vista-cajero-titulo">Vista Cajero</h1>
      <p>Espacio reservado para la operación de ventanillas, fila FIFO e historial (#09).</p>
      {estado && (
        <div className="sgt-placeholder__resumen">
          {estado.ventanillas.map((ventanilla) => (
            <div key={ventanilla.id} className="sgt-placeholder__item">
              <strong>{ventanilla.id}</strong>
              <EstadoBadge tipo="ventanilla" ventanilla={ventanilla} />
            </div>
          ))}
        </div>
      )}
    </section>
  );
}
