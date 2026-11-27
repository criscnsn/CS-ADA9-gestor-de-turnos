import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { NotificacionBanner } from "./NotificacionBanner";

afterEach(() => cleanup());

describe("NotificacionBanner (RF-12, RNF-U02)", () => {
  it("muestra error con causa, código e ID implicado y permite cerrarlo", () => {
    const onCerrar = vi.fn();
    render(
      <NotificacionBanner
        notificacion={{
          tipo: "error",
          codigo: "VENTANILLA_OCUPADA",
          mensaje: "La ventanilla V02 está ocupada.",
          recursoId: "V02",
        }}
        onCerrar={onCerrar}
      />,
    );

    const alerta = screen.getByRole("alert");
    expect(alerta.textContent).toContain("Error:");
    expect(alerta.textContent).toContain("La ventanilla V02 está ocupada.");
    expect(screen.getByTestId("notificacion-recurso-id").textContent).toContain("V02");
    expect(screen.getByTestId("notificacion-codigo").textContent).toContain("VENTANILLA_OCUPADA");

    fireEvent.click(screen.getByRole("button", { name: "Cerrar notificación" }));
    expect(onCerrar).toHaveBeenCalledTimes(1);
  });

  it("muestra confirmación de éxito con role status", () => {
    render(
      <NotificacionBanner
        notificacion={{
          tipo: "exito",
          mensaje: "Turno T-0001 registrado en la fila de espera.",
          recursoId: "T-0001",
        }}
      />,
    );

    const status = screen.getByRole("status");
    expect(status.textContent).toContain("Confirmación:");
    expect(status.textContent).toContain("T-0001");
  });
});
