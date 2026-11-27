import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { App } from "./App";
import { createMockSgtClient } from "./services/mockSgtClient";
import type { SgtClient } from "./services/sgtClient";
import type { AppError } from "./types/sgt";

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe("App — selector de vistas, query params y error crítico", () => {
  it("muestra el selector inicial sin query params, navega entre vistas y llama abrir_ventana", async () => {
    const mock = createMockSgtClient();
    const spyAbrir = vi.spyOn(mock, "abrir_ventana");

    render(<App client={mock} initialSearch="" />);

    await waitFor(() =>
      expect(screen.getByRole("heading", { name: /Sistema de Gestión de Turnos/i })).toBeTruthy(),
    );

    fireEvent.click(screen.getByRole("button", { name: "Abrir ventana Cajero" }));
    await waitFor(() => expect(spyAbrir).toHaveBeenCalledWith({ vista: "cajero" }));

    fireEvent.click(screen.getByRole("button", { name: "Entrar como Cliente" }));
    expect(screen.getByRole("heading", { name: "Vista Cliente" })).toBeTruthy();

    fireEvent.click(screen.getByRole("button", { name: "Cambiar vista" }));
    expect(screen.getByRole("heading", { name: /Sistema de Gestión de Turnos/i })).toBeTruthy();
  });

  it("abre directamente Vista Cliente o Vista Cajero según ?vista=cliente / ?vista=cajero", async () => {
    const { unmount } = render(
      <App client={createMockSgtClient()} initialSearch="?vista=cajero" />,
    );
    await waitFor(() => expect(screen.getByRole("heading", { name: "Vista Cajero" })).toBeTruthy());
    unmount();

    render(<App client={createMockSgtClient()} initialSearch="index.html?vista=cliente" />);
    await waitFor(() =>
      expect(screen.getByRole("heading", { name: "Vista Cliente" })).toBeTruthy(),
    );
  });

  it("muestra ErrorBaseDatosView cuando el backend reporta BASE_DATOS_CORRUPTA (RD-03)", async () => {
    const errorBd: AppError = {
      codigo: "BASE_DATOS_CORRUPTA",
      mensaje: "Archivo de base de datos ilegible.",
      recursoId: null,
    };

    const clientCorrupto: SgtClient = {
      ...createMockSgtClient(),
      escuchar_estado: vi.fn(async () => () => {}),
      obtener_estado: vi.fn(async () => {
        throw errorBd;
      }),
    };

    render(<App client={clientCorrupto} initialSearch="" />);

    await waitFor(() => expect(screen.getByRole("alert")).toBeTruthy());
    expect(screen.getByRole("alert").textContent).toContain("BASE_DATOS_CORRUPTA");
    expect(screen.getByRole("alert").textContent).toContain("Archivo de base de datos ilegible.");
  });
});
