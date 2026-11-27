import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { useState } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { createMockSgtClient } from "../services/mockSgtClient";
import { SgtProvider } from "../state/SgtProvider";
import { useSgt } from "../state/sgtContext";
import { ConfirmacionModal } from "./ConfirmacionModal";

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

function ConsumidorCancelacion({ turnoId }: { turnoId: string }) {
  const { cargando, cancelarTurno } = useSgt();
  const [abierto, setAbierto] = useState(false);

  if (cargando) {
    return <p>Cargando...</p>;
  }

  return (
    <div>
      <button type="button" onClick={() => setAbierto(true)}>
        Cancelar
      </button>
      <ConfirmacionModal
        abierto={abierto}
        titulo="Confirmar cancelación"
        mensaje={`¿Deseas cancelar el turno ${turnoId}?`}
        textoConfirmar="Sí, cancelar"
        textoCancelar="No cancelar"
        onCancelar={() => setAbierto(false)}
        onConfirmar={async () => {
          await cancelarTurno(turnoId);
          setAbierto(false);
        }}
      />
    </div>
  );
}

describe("ConfirmacionModal (RNF-U04)", () => {
  it("rechazar el modal por botón o tecla Escape NO dispara cancelar_turno y conserva el turno EN_ESPERA", async () => {
    const mock = createMockSgtClient({ now: () => new Date("2026-11-18T01:00:00.000Z") });
    const turno = await mock.registrar_turno();
    const spyCancelar = vi.spyOn(mock, "cancelar_turno");

    render(
      <SgtProvider client={mock}>
        <ConsumidorCancelacion turnoId={turno.id} />
      </SgtProvider>,
    );

    const botonCancelar = await screen.findByRole("button", { name: "Cancelar" });

    // Abrir y rechazar con botón secundario.
    fireEvent.click(botonCancelar);
    expect(screen.getByRole("dialog")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "No cancelar" }));
    expect(screen.queryByRole("dialog")).toBeNull();

    // Abrir y rechazar con tecla Escape.
    fireEvent.click(botonCancelar);
    expect(screen.getByRole("dialog")).toBeTruthy();
    fireEvent.keyDown(window, { key: "Escape" });
    expect(screen.queryByRole("dialog")).toBeNull();

    expect(spyCancelar).not.toHaveBeenCalled();
    const estadoActual = await mock.obtener_estado();
    expect(estadoActual.filaEspera).toHaveLength(1);
    expect(estadoActual.filaEspera[0].id).toBe("T-0001");
    expect(estadoActual.filaEspera[0].estado).toBe("EN_ESPERA");
  });

  it("confirmar en el modal ejecuta cancelar_turno correctamente", async () => {
    const mock = createMockSgtClient({ now: () => new Date("2026-11-18T01:00:00.000Z") });
    const turno = await mock.registrar_turno();
    const spyCancelar = vi.spyOn(mock, "cancelar_turno");

    render(
      <SgtProvider client={mock}>
        <ConsumidorCancelacion turnoId={turno.id} />
      </SgtProvider>,
    );

    const botonCancelar = await screen.findByRole("button", { name: "Cancelar" });
    fireEvent.click(botonCancelar);
    fireEvent.click(screen.getByRole("button", { name: "Sí, cancelar" }));

    await waitFor(() => expect(spyCancelar).toHaveBeenCalledWith({ turnoId: "T-0001" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    const estadoActual = await mock.obtener_estado();
    expect(estadoActual.filaEspera).toHaveLength(0);
    expect(estadoActual.historial[0].estado).toBe("CANCELADO");
  });
});
