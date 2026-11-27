import { EstadoBadge } from "../components/EstadoBadge";
import { useSgt } from "../state/sgtContext";

/**
 * Contenedor base para Vista Cliente (la interfaz completa se implementa en `#10`).
 */
export function TurnosPage() {
  const { estado } = useSgt();

  return (
    <section className="sgt-placeholder" aria-labelledby="vista-cliente-titulo">
      <h1 id="vista-cliente-titulo">Vista Cliente</h1>
      <p>Espacio reservado para la sala de espera y matriz de turnos (#10).</p>
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
