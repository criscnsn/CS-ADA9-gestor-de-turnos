import { act, cleanup, render, renderHook, waitFor } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import mixto from "../../docs/fixtures/contrato/estado-mixto.json";
import vacio from "../../docs/fixtures/contrato/estado-vacio.json";
import { createMockSgtClient } from "../services/mockSgtClient";
import type { EscucharEstado, SgtClient } from "../services/sgtClient";
import type { AppError, EstadoSistema } from "../types/sgt";
import { SgtProvider } from "./SgtProvider";
import { useSgt } from "./sgtContext";

const ESTADO_VACIO = vacio as unknown as EstadoSistema;
const ESTADO_MIXTO = mixto as unknown as EstadoSistema;

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

function crearWrapper(client?: SgtClient) {
  return function Wrapper({ children }: { children: ReactNode }) {
    return <SgtProvider client={client}>{children}</SgtProvider>;
  };
}

describe("SgtProvider y useSgt", () => {
  it("lanza error claro si useSgt se usa fuera de SgtProvider", () => {
    const spy = vi.spyOn(console, "error").mockImplementation(() => {});
    expect(() => renderHook(() => useSgt())).toThrow(/SgtProvider/);
    spy.mockRestore();
  });

  it("hidrata estado inicial desde el mock por defecto y ejecuta operaciones con notificaciones", async () => {
    const mock = createMockSgtClient({ now: () => new Date("2026-11-18T01:00:00.000Z") });
    const { result } = renderHook(() => useSgt(), { wrapper: crearWrapper(mock) });

    await waitFor(() => expect(result.current.cargando).toBe(false));
    expect(result.current.estado).toEqual(ESTADO_VACIO);
    expect(result.current.errorCritico).toBeNull();
    expect(result.current.notificacion).toBeNull();

    // Registrar dos turnos (RF-02).
    let resRegistro = await act(async () => result.current.registrarTurno());
    expect(resRegistro.ok).toBe(true);
    if (resRegistro.ok) {
      expect(resRegistro.data.id).toBe("T-0001");
    }
    expect(result.current.notificacion).toEqual({
      tipo: "exito",
      mensaje: "Turno T-0001 registrado en la fila de espera.",
      recursoId: "T-0001",
    });

    resRegistro = await act(async () => result.current.registrarTurno());
    expect(resRegistro.ok).toBe(true);
    expect(result.current.estado?.filaEspera.map((t) => t.id)).toEqual(["T-0001", "T-0002"]);

    // Asignar siguiente en V02 (RF-04).
    const resAsignar = await act(async () => result.current.asignarSiguiente("V02"));
    expect(resAsignar.ok).toBe(true);
    expect(result.current.estado?.ventanillas[1].turnoActualId).toBe("T-0001");
    expect(result.current.notificacion).toEqual({
      tipo: "exito",
      mensaje: "Turno T-0001 asignado a la ventanilla V02.",
      recursoId: "T-0001",
    });

    // Finalizar atención en V02 (RF-06).
    const resFinalizar = await act(async () => result.current.finalizarAtencion("V02"));
    expect(resFinalizar.ok).toBe(true);
    expect(result.current.estado?.ventanillas[1].turnoActualId).toBeNull();
    expect(result.current.notificacion?.mensaje).toContain("T-0001");
    expect(result.current.notificacion?.mensaje).toContain("V02");

    // Cancelar T-0002 en espera (RF-08).
    const resCancelar = await act(async () => result.current.cancelarTurno("T-0002"));
    expect(resCancelar.ok).toBe(true);
    expect(result.current.estado?.filaEspera).toHaveLength(0);
    expect(result.current.estado?.historial).toHaveLength(2);

    // Inhabilitar V03 (RF-13).
    const resInhabilitar = await act(async () =>
      result.current.cambiarEstadoVentanilla("V03", false),
    );
    expect(resInhabilitar.ok).toBe(true);
    expect(result.current.estado?.ventanillas[2].habilitada).toBe(false);
    expect(result.current.notificacion).toEqual({
      tipo: "exito",
      mensaje: "Ventanilla V03 inhabilitada.",
      recursoId: "V03",
    });

    // abrirVentana válido no falla y limpiarNotificacion limpia la alerta.
    const resVentana = await act(async () => result.current.abrirVentana("cliente"));
    expect(resVentana.ok).toBe(true);

    act(() => result.current.limpiarNotificacion());
    expect(result.current.notificacion).toBeNull();
  });

  it("sincroniza cambios reactivos emitidos por otro consumidor del mismo SgtClient", async () => {
    const mock = createMockSgtClient({ now: () => new Date("2026-11-18T01:00:00.000Z") });
    const { result } = renderHook(() => useSgt(), { wrapper: crearWrapper(mock) });

    await waitFor(() => expect(result.current.cargando).toBe(false));
    expect(result.current.estado?.filaEspera).toHaveLength(0);

    // Simula otra ventana registrando un turno en el mismo backend/cliente.
    await act(async () => {
      await mock.registrar_turno();
    });

    await waitFor(() => expect(result.current.estado?.filaEspera).toHaveLength(1));
    expect(result.current.estado?.filaEspera[0].id).toBe("T-0001");
  });

  it("expone AppError operativo en notificacion sin activar errorCritico ni mutar estado", async () => {
    const mock = createMockSgtClient({ now: () => new Date("2026-11-18T01:00:00.000Z") });
    const { result } = renderHook(() => useSgt(), { wrapper: crearWrapper(mock) });

    await waitFor(() => expect(result.current.cargando).toBe(false));
    const snapshotPrevio = structuredClone(result.current.estado);

    const res = await act(async () => result.current.asignarSiguiente("V01"));
    expect(res.ok).toBe(false);
    if (!res.ok) {
      expect(res.error.codigo).toBe("FILA_VACIA");
      expect(res.error.recursoId).toBe("V01");
    }

    expect(result.current.errorCritico).toBeNull();
    expect(result.current.notificacion).toEqual({
      tipo: "error",
      codigo: "FILA_VACIA",
      mensaje: expect.stringContaining("V01"),
      recursoId: "V01",
    });
    expect(result.current.estado).toEqual(snapshotPrevio);
  });

  it("activa errorCritico ante BASE_DATOS_CORRUPTA en carga inicial y permite recargarEstado", async () => {
    const errorBd: AppError = {
      codigo: "BASE_DATOS_CORRUPTA",
      mensaje: "El archivo sgt.db está dañado y no se sobrescribirá automáticamente.",
      recursoId: null,
    };

    let fallar = true;
    const clientDoble: SgtClient = {
      ...createMockSgtClient(),
      obtener_estado: vi.fn(async () => {
        if (fallar) throw errorBd;
        return structuredClone(ESTADO_MIXTO);
      }),
      escuchar_estado: vi.fn(async () => () => {}),
    };

    const { result } = renderHook(() => useSgt(), { wrapper: crearWrapper(clientDoble) });

    await waitFor(() => expect(result.current.cargando).toBe(false));
    expect(result.current.errorCritico).toEqual(errorBd);
    expect(result.current.estado).toBeNull();
    expect(result.current.notificacion).toBeNull();

    fallar = false;
    const recarga = await act(async () => result.current.recargarEstado());
    expect(recarga.ok).toBe(true);
    expect(result.current.errorCritico).toBeNull();
    expect(result.current.estado).toEqual(ESTADO_MIXTO);
  });

  it("evita sobrescribir un evento en vivo más reciente con una hidratación tardía (RI-02)", async () => {
    let emitirEvento: EscucharEstado | null = null;
    let resolverInicial: ((estado: EstadoSistema) => void) | null = null;

    const clientLento: SgtClient = {
      ...createMockSgtClient(),
      escuchar_estado: vi.fn(async (listener) => {
        emitirEvento = listener;
        return () => {};
      }),
      obtener_estado: vi.fn(
        () =>
          new Promise<EstadoSistema>((resolve) => {
            resolverInicial = resolve;
          }),
      ),
    };

    const { result } = renderHook(() => useSgt(), { wrapper: crearWrapper(clientLento) });

    await waitFor(() => expect(emitirEvento).not.toBeNull());
    await waitFor(() => expect(resolverInicial).not.toBeNull());

    // Llega un evento más reciente antes de que termine obtener_estado().
    act(() => {
      emitirEvento?.(structuredClone(ESTADO_MIXTO));
    });

    // Ahora resuelve la consulta inicial más vieja; debe prevalecer ESTADO_MIXTO.
    await act(async () => {
      resolverInicial?.(structuredClone(ESTADO_VACIO));
    });
    await waitFor(() => expect(result.current.cargando).toBe(false));
    expect(result.current.estado).toEqual(ESTADO_MIXTO);
  });

  it("ejecuta desuscribir al desmontar incluso si escuchar_estado resuelve después", async () => {
    const desuscribir = vi.fn();
    let resolverEscucha: ((off: () => void) => void) | null = null;

    const clientAsincrono: SgtClient = {
      ...createMockSgtClient(),
      escuchar_estado: vi.fn(
        () =>
          new Promise<() => void>((resolve) => {
            resolverEscucha = resolve;
          }),
      ),
    };

    const { unmount } = render(
      <SgtProvider client={clientAsincrono}>
        <div>consumidor</div>
      </SgtProvider>,
    );

    await waitFor(() => expect(resolverEscucha).not.toBeNull());
    unmount();

    await act(async () => {
      resolverEscucha?.(desuscribir);
    });

    await waitFor(() => expect(desuscribir).toHaveBeenCalledTimes(1));
  });
});
