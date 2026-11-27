import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ErrorBaseDatosView } from "./ErrorBaseDatosView";

afterEach(() => cleanup());

describe("ErrorBaseDatosView (RD-03)", () => {
  it("muestra aviso visible de BASE_DATOS_CORRUPTA sin sobrescribir datos y permite reintentar", () => {
    const onReintentar = vi.fn();
    render(
      <ErrorBaseDatosView
        error={{
          codigo: "BASE_DATOS_CORRUPTA",
          mensaje: "PRAGMA integrity_check falló sobre sgt.db.",
          recursoId: "sgt.db",
        }}
        onReintentar={onReintentar}
      />,
    );

    const alerta = screen.getByRole("alert");
    expect(alerta.textContent).toContain("BASE_DATOS_CORRUPTA");
    expect(alerta.textContent).toContain("PRAGMA integrity_check falló sobre sgt.db.");
    expect(alerta.textContent).toContain("sgt.db");
    expect(alerta.textContent).toContain("no sobrescribió");

    fireEvent.click(screen.getByRole("button", { name: "Reintentar carga" }));
    expect(onReintentar).toHaveBeenCalledTimes(1);
  });
});
