import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { EstadoBadge } from "./EstadoBadge";
import { derivarEstadoVentanilla } from "./estadoLabels";

afterEach(() => cleanup());

describe("EstadoBadge (RNF-U03)", () => {
  it("muestra texto explícito y atributos accesibles para los 4 estados de turno", () => {
    const casos = [
      { estado: "EN_ESPERA" as const, texto: "En espera" },
      { estado: "EN_ATENCION" as const, texto: "En atención" },
      { estado: "ATENDIDO" as const, texto: "Atendido" },
      { estado: "CANCELADO" as const, texto: "Cancelado" },
    ];

    for (const caso of casos) {
      const { unmount } = render(<EstadoBadge tipo="turno" estado={caso.estado} />);
      const badge = screen.getByLabelText(`Estado: ${caso.texto}`);
      expect(badge.textContent).toContain(caso.texto);
      expect(badge.getAttribute("data-estado")).toBe(caso.estado);
      unmount();
    }
  });

  it("deriva y muestra texto explícito para Libre, Ocupado e Inhabilitado", () => {
    const libre = { id: "V01" as const, habilitada: true, turnoActualId: null };
    const ocupado = { id: "V02" as const, habilitada: true, turnoActualId: "T-0001" };
    const inhabilitado = { id: "V03" as const, habilitada: false, turnoActualId: null };

    expect(derivarEstadoVentanilla(libre)).toBe("LIBRE");
    expect(derivarEstadoVentanilla(ocupado)).toBe("OCUPADO");
    expect(derivarEstadoVentanilla(inhabilitado)).toBe("INHABILITADO");

    render(
      <div>
        <EstadoBadge tipo="ventanilla" ventanilla={libre} />
        <EstadoBadge tipo="ventanilla" ventanilla={ocupado} />
        <EstadoBadge tipo="ventanilla" ventanilla={inhabilitado} />
      </div>,
    );

    expect(screen.getByText("Libre")).toBeTruthy();
    expect(screen.getByText("Ocupado")).toBeTruthy();
    expect(screen.getByText("Inhabilitado")).toBeTruthy();
  });
});
