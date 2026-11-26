import { afterEach, describe, expect, it, vi } from "vitest";
import { createMockSgtClient } from "./mockSgtClient";
import type { EstadoSistema } from "../types/sgt";

const create = () => createMockSgtClient({ now: () => new Date("2026-11-18T01:00:00.000Z") });
afterEach(() => vi.restoreAllMocks());

describe("mock de turnos — casos borde", () => {
  it("encadena mutaciones reentrantes en orden FIFO estricto de snapshots", async () => {
    const client = create();
    const snapshots: Array<{
      fila: string[];
      v01Turno: string | null;
      ultimoEstado: string | undefined;
      ventanillaAsignada: string | null | undefined;
    }> = [];
    const operaciones: Promise<unknown>[] = [];

    await client.escuchar_estado((estado) => {
      const turno = estado.historial[0];
      if (turno?.estado === "EN_ESPERA") {
        operaciones.push(client.asignar_siguiente({ ventanillaId: "V01" }));
      } else if (turno?.estado === "EN_ATENCION") {
        operaciones.push(client.finalizar_atencion({ ventanillaId: "V01" }));
      }
    });

    await client.escuchar_estado((estado) => {
      snapshots.push({
        fila: estado.filaEspera.map((t) => t.id),
        v01Turno: estado.ventanillas[0].turnoActualId,
        ultimoEstado: estado.historial[0]?.estado,
        ventanillaAsignada: estado.historial[0]?.ventanillaAsignada,
      });
    });

    await client.registrar_turno();
    await Promise.all(operaciones);

    expect(snapshots).toEqual([
      {
        fila: ["T-0001"],
        v01Turno: null,
        ultimoEstado: "EN_ESPERA",
        ventanillaAsignada: null,
      },
      {
        fila: [],
        v01Turno: "T-0001",
        ultimoEstado: "EN_ATENCION",
        ventanillaAsignada: "V01",
      },
      {
        fila: [],
        v01Turno: null,
        ultimoEstado: "ATENDIDO",
        ventanillaAsignada: "V01",
      },
    ]);
  });

  it("una mutación reentrante rechazada no corrompe la memoria ni bloquea futuras emisiones", async () => {
    const client = create();
    const observados: string[] = [];
    let falloReentrante: Promise<unknown> | undefined;

    await client.escuchar_estado((estado) => {
      observados.push(`${estado.historial.length}:${estado.historial[0]?.estado ?? "-"}`);
      if (estado.historial.length === 1 && estado.historial[0].estado === "EN_ESPERA") {
        falloReentrante = client.cancelar_turno({ turnoId: "T-9999" });
      }
    });

    const turno = await client.registrar_turno();
    await expect(falloReentrante).rejects.toMatchObject({
      codigo: "TURNO_INEXISTENTE",
      recursoId: "T-9999",
    });

    await client.cancelar_turno({ turnoId: turno.id });
    expect(observados).toEqual(["1:EN_ESPERA", "1:CANCELADO"]);
  });

  it("aísla el rechazo asíncrono de un listener sin unhandledRejection ni afectar comando ni otros listeners", async () => {
    const client = create();
    const errorAsync = new Error("listener async roto");
    const report = vi.spyOn(console, "error").mockImplementation(() => {});
    const onUnhandled = vi.fn();
    const onWindowUnhandled = vi.fn();
    const otroListener = vi.fn();
    const nodeProcess = (
      globalThis as {
        process?: {
          on(event: "unhandledRejection", listener: (reason: unknown) => void): void;
          off(event: "unhandledRejection", listener: (reason: unknown) => void): void;
        };
      }
    ).process;

    nodeProcess?.on("unhandledRejection", onUnhandled);
    window.addEventListener("unhandledrejection", onWindowUnhandled);
    try {
      await client.escuchar_estado(async () => {
        await Promise.resolve();
        throw errorAsync;
      });
      await client.escuchar_estado(otroListener);

      await expect(client.registrar_turno()).resolves.toMatchObject({
        id: "T-0001",
        estado: "EN_ESPERA",
      });

      // Esperar a que la promesa rechazada del listener y su .catch() drenen.
      await new Promise((resolve) => setTimeout(resolve, 0));

      expect(otroListener).toHaveBeenCalledTimes(1);
      expect(otroListener.mock.calls[0][0].filaEspera.map((t: { id: string }) => t.id)).toEqual([
        "T-0001",
      ]);
      expect(report).toHaveBeenCalledTimes(1);
      expect(report).toHaveBeenCalledWith("Error en un listener del mock SGT:", errorAsync);
      expect(onUnhandled).not.toHaveBeenCalled();
      expect(onWindowUnhandled).not.toHaveBeenCalled();
      expect((await client.obtener_estado()).historial).toHaveLength(1);
    } finally {
      nodeProcess?.off("unhandledRejection", onUnhandled);
      window.removeEventListener("unhandledrejection", onWindowUnhandled);
      report.mockRestore();
    }
  });

  it("desuscribir a otro listener durante una emisión evita su llamada actual y posteriores", async () => {
    const client = create();
    const listenerA = vi.fn();
    const listenerB = vi.fn();
    const listenerC = vi.fn();

    await client.escuchar_estado((estado) => {
      listenerA(estado.historial.length);
      offB();
    });
    const offB = await client.escuchar_estado((estado) => {
      listenerB(estado.historial.length);
    });
    await client.escuchar_estado((estado) => {
      listenerC(estado.historial.length);
    });

    const turno = await client.registrar_turno();
    await client.cancelar_turno({ turnoId: turno.id });

    expect(listenerA).toHaveBeenCalledTimes(2);
    expect(listenerB).not.toHaveBeenCalled();
    expect(listenerC).toHaveBeenCalledTimes(2);
  });

  it("un listener suscrito durante una emisión no recibe el evento actual ni eventos ya encolados", async () => {
    const client = create();
    const tardioTrasEncolar = vi.fn();
    const tardioAntesDeEncolar = vi.fn();
    const pendientes: Promise<unknown>[] = [];
    let configurado = false;

    await client.escuchar_estado(() => {
      if (configurado) return;
      configurado = true;
      // Suscrito antes de encolar la mutación reentrante: omite evento 1, recibe evento 2 y 3.
      pendientes.push(client.escuchar_estado(tardioAntesDeEncolar));
      pendientes.push(client.registrar_turno());
      // Suscrito después de encolar la mutación reentrante: omite evento 1 y 2, solo recibe evento 3.
      pendientes.push(client.escuchar_estado(tardioTrasEncolar));
    });

    await client.registrar_turno();
    await Promise.all(pendientes);
    expect(tardioAntesDeEncolar).toHaveBeenCalledTimes(1);
    expect(tardioTrasEncolar).not.toHaveBeenCalled();

    await client.registrar_turno();
    expect(tardioAntesDeEncolar).toHaveBeenCalledTimes(2);
    expect(tardioTrasEncolar).toHaveBeenCalledTimes(1);
  });

  it("auto-desuscribirse durante un evento omite notificaciones reentrantes ya encoladas", async () => {
    const client = create();
    const recibidosOneShot: number[] = [];
    const recibidosObserver: number[] = [];
    let reentrante: Promise<unknown> | undefined;

    const offOneShot = await client.escuchar_estado((estado) => {
      recibidosOneShot.push(estado.historial.length);
      reentrante = client.registrar_turno();
      offOneShot();
    });
    await client.escuchar_estado((estado) => {
      recibidosObserver.push(estado.historial.length);
    });

    await client.registrar_turno();
    await reentrante;

    expect(recibidosOneShot).toEqual([1]);
    expect(recibidosObserver).toEqual([1, 2]);
  });

  it("aísla mutaciones externas sobre los retornos de todos los comandos y arrays del estado", async () => {
    const client = create();
    const snapshots: EstadoSistema[] = [];
    await client.escuchar_estado((estado) => snapshots.push(estado));

    const t1 = await client.registrar_turno();
    const t2 = await client.registrar_turno();

    const asignado = await client.asignar_siguiente({ ventanillaId: "V01" });
    asignado.estado = "ATENDIDO";
    asignado.ventanillaAsignada = "V03";
    asignado.id = "alterado";

    expect((await client.obtener_estado()).historial[0]).toMatchObject({
      id: t1.id,
      estado: "EN_ATENCION",
      ventanillaAsignada: "V01",
    });

    const finalizado = await client.finalizar_atencion({ ventanillaId: "V01" });
    finalizado.estado = "CANCELADO";
    finalizado.ventanillaAsignada = null;

    const cancelado = await client.cancelar_turno({ turnoId: t2.id });
    cancelado.estado = "EN_ESPERA";

    const ventanilla = await client.cambiar_estado_ventanilla({
      ventanillaId: "V02",
      habilitada: false,
    });
    ventanilla.habilitada = true;
    ventanilla.turnoActualId = "T-9999";

    const estadoLeido = await client.obtener_estado();
    estadoLeido.filaEspera.push({
      id: "T-FAKE",
      secuencia: 99,
      fechaHoraRegistro: "2026-11-18T01:00:00.000Z",
      estado: "EN_ESPERA",
      ventanillaAsignada: null,
    });

    const estadoReal = await client.obtener_estado();
    expect(estadoReal.filaEspera).toEqual([]);
    expect(estadoReal.historial).toMatchObject([
      { id: t1.id, estado: "ATENDIDO", ventanillaAsignada: "V01" },
      { id: t2.id, estado: "CANCELADO", ventanillaAsignada: null },
    ]);
    expect(estadoReal.ventanillas[1]).toEqual({
      id: "V02",
      habilitada: false,
      turnoActualId: null,
    });
    expect(snapshots[snapshots.length - 1]).toEqual(estadoReal);
  });

  it("rechaza argumentos externos malformados y normaliza IDs en todos los comandos", async () => {
    const client = create();
    const listener = vi.fn();
    await client.escuchar_estado(listener);

    // @ts-expect-error Validar tipo de listener en runtime.
    await expect(client.escuchar_estado(null)).rejects.toMatchObject({
      codigo: "ENTRADA_INVALIDA",
      recursoId: null,
    });
    // @ts-expect-error Validar tipo de listener en runtime.
    await expect(client.escuchar_estado("no-es-funcion")).rejects.toMatchObject({
      codigo: "ENTRADA_INVALIDA",
      recursoId: null,
    });
    // @ts-expect-error Validar argumentos ausentes en abrir_ventana.
    await expect(client.abrir_ventana()).rejects.toMatchObject({
      codigo: "ENTRADA_INVALIDA",
      recursoId: null,
    });
    await expect(
      // @ts-expect-error vista no debe aceptar espacios alrededor.
      client.abrir_ventana({ vista: " cliente " }),
    ).rejects.toMatchObject({
      codigo: "ENTRADA_INVALIDA",
      recursoId: null,
    });
    // @ts-expect-error Validar argumentos ausentes en asignar_siguiente.
    await expect(client.asignar_siguiente()).rejects.toMatchObject({
      codigo: "ENTRADA_INVALIDA",
      recursoId: null,
    });
    // @ts-expect-error Validar argumentos ausentes en cancelar_turno.
    await expect(client.cancelar_turno()).rejects.toMatchObject({
      codigo: "ENTRADA_INVALIDA",
      recursoId: null,
    });
    // @ts-expect-error Validar argumentos ausentes en cambiar_estado_ventanilla.
    await expect(client.cambiar_estado_ventanilla()).rejects.toMatchObject({
      codigo: "ENTRADA_INVALIDA",
      recursoId: null,
    });
    await expect(
      // @ts-expect-error Validar habilitada ausente tras validar ventanilla existente.
      client.cambiar_estado_ventanilla({ ventanillaId: "V01" }),
    ).rejects.toMatchObject({
      codigo: "ENTRADA_INVALIDA",
      recursoId: "V01",
    });
    await expect(
      // @ts-expect-error Comprobar que recursoId reportado llega recortado.
      client.asignar_siguiente({ ventanillaId: " V99 " }),
    ).rejects.toMatchObject({
      codigo: "VENTANILLA_INVALIDA",
      recursoId: "V99",
    });
    await expect(client.cancelar_turno({ turnoId: " T-9999 " })).rejects.toMatchObject({
      codigo: "TURNO_INEXISTENTE",
      recursoId: "T-9999",
    });
    expect(listener).not.toHaveBeenCalled();

    const turno = await client.registrar_turno();
    // @ts-expect-error Validar trim en tiempo de ejecución.
    await expect(client.asignar_siguiente({ ventanillaId: " V01 " })).resolves.toMatchObject({
      id: turno.id,
      ventanillaAsignada: "V01",
    });
    // @ts-expect-error Validar trim en tiempo de ejecución.
    await expect(client.finalizar_atencion({ ventanillaId: " V01 " })).resolves.toMatchObject({
      id: turno.id,
      estado: "ATENDIDO",
    });
    await expect(
      // @ts-expect-error Validar trim en tiempo de ejecución.
      client.cambiar_estado_ventanilla({ ventanillaId: " V03 ", habilitada: false }),
    ).resolves.toEqual({ id: "V03", habilitada: false, turnoActualId: null });
  });

  it("respeta precedencia de errores, habilitación idempotente en ocupada y desplazamiento FIFO al cancelar cabeza", async () => {
    const client = create();

    // Ventanilla inhabilitada con fila vacía debe reportar VENTANILLA_INHABILITADA antes que FILA_VACIA.
    await client.cambiar_estado_ventanilla({ ventanillaId: "V02", habilitada: false });
    await expect(client.asignar_siguiente({ ventanillaId: "V02" })).rejects.toMatchObject({
      codigo: "VENTANILLA_INHABILITADA",
      recursoId: "V02",
    });

    const t1 = await client.registrar_turno();
    const t2 = await client.registrar_turno();
    const t3 = await client.registrar_turno();

    // Cancelar la cabeza de la cola desplaza el siguiente turno FIFO sin reordenar historial.
    await client.cancelar_turno({ turnoId: t1.id });
    expect((await client.obtener_estado()).filaEspera.map((t) => t.id)).toEqual([t2.id, t3.id]);

    await client.asignar_siguiente({ ventanillaId: "V01" });
    expect((await client.obtener_estado()).filaEspera.map((t) => t.id)).toEqual([t3.id]);

    // Inhabilitar la única ventanilla habilitada mientras está ocupada reporta VENTANILLA_OCUPADA.
    await client.cambiar_estado_ventanilla({ ventanillaId: "V03", habilitada: false });
    await expect(
      client.cambiar_estado_ventanilla({ ventanillaId: "V01", habilitada: false }),
    ).rejects.toMatchObject({
      codigo: "VENTANILLA_OCUPADA",
      recursoId: "V01",
    });

    // Reconfirmar habilitada: true sobre una ventanilla ocupada es válido y emite snapshot.
    const listener = vi.fn();
    const off = await client.escuchar_estado(listener);
    await expect(
      client.cambiar_estado_ventanilla({ ventanillaId: "V01", habilitada: true }),
    ).resolves.toEqual({ id: "V01", habilitada: true, turnoActualId: t2.id });
    expect(listener).toHaveBeenCalledTimes(1);
    off();

    // Al finalizar atención, la ventanilla queda libre (turnoActualId: null) y el historial conserva asignación.
    await client.finalizar_atencion({ ventanillaId: "V01" });
    const estado = await client.obtener_estado();
    expect(estado.ventanillas[0]).toEqual({ id: "V01", habilitada: true, turnoActualId: null });
    expect(estado.historial.find((t) => t.id === t2.id)).toMatchObject({
      estado: "ATENDIDO",
      ventanillaAsignada: "V01",
    });
  });
});
