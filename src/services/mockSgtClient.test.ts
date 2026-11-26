import { afterEach, describe, expect, it, vi } from "vitest";
import vacio from "../../docs/fixtures/contrato/estado-vacio.json";
import { createMockSgtClient } from "./mockSgtClient";
import { EVENTO_ESTADO_ACTUALIZADO } from "./sgtClient";
import type { SgtClient } from "./sgtClient";
import type { CodigoError, EstadoSistema } from "../types/sgt";

const create = () => createMockSgtClient({ now: () => new Date("2026-11-18T01:00:00.000Z") });
afterEach(() => vi.restoreAllMocks());

async function sinCambios(
  client: SgtClient,
  operacion: () => Promise<unknown>,
  codigo: CodigoError,
  recursoId: string | null,
) {
  const antes = await client.obtener_estado();
  const listener = vi.fn();
  const off = await client.escuchar_estado(listener);
  await expect(operacion()).rejects.toMatchObject({
    codigo,
    recursoId,
    mensaje: expect.any(String),
  });
  expect(await client.obtener_estado()).toEqual(antes);
  expect(listener).not.toHaveBeenCalled();
  off();
}

describe("mock de turnos", () => {
  it("empieza vacío con tres ventanillas habilitadas y no autoasigna al registrar", async () => {
    const client = create();
    expect(await client.obtener_estado()).toEqual(vacio);
    expect(await client.registrar_turno()).toEqual({
      id: "T-0001",
      secuencia: 1,
      fechaHoraRegistro: "2026-11-18T01:00:00.000Z",
      estado: "EN_ESPERA",
      ventanillaAsignada: null,
    });
    expect((await client.obtener_estado()).ventanillas).toEqual(vacio.ventanillas);
  });

  it("asigna FIFO en V02 y V01, finaliza y conserva el historial cancelado/atendido", async () => {
    const client = create();
    const [t1, t2, t3, t4] = await Promise.all(
      Array.from({ length: 4 }, () => client.registrar_turno()),
    );
    expect([t1.id, t2.id, t3.id, t4.id]).toEqual(["T-0001", "T-0002", "T-0003", "T-0004"]);
    expect(await client.asignar_siguiente({ ventanillaId: "V02" })).toMatchObject({
      id: t1.id,
      estado: "EN_ATENCION",
      ventanillaAsignada: "V02",
    });
    expect(await client.asignar_siguiente({ ventanillaId: "V01" })).toMatchObject({
      id: t2.id,
      ventanillaAsignada: "V01",
    });
    expect(await client.cancelar_turno({ turnoId: ` ${t3.id} ` })).toMatchObject({
      id: t3.id,
      estado: "CANCELADO",
    });
    expect(await client.finalizar_atencion({ ventanillaId: "V02" })).toMatchObject({
      id: t1.id,
      estado: "ATENDIDO",
      ventanillaAsignada: "V02",
    });
    expect(await client.asignar_siguiente({ ventanillaId: "V02" })).toMatchObject({ id: t4.id });
    const estado = await client.obtener_estado();
    expect(estado.filaEspera).toEqual([]);
    expect(estado.historial.map((t) => t.estado)).toEqual([
      "ATENDIDO",
      "EN_ATENCION",
      "CANCELADO",
      "EN_ATENCION",
    ]);
    expect(await client.registrar_turno()).toMatchObject({ id: "T-0005", secuencia: 5 });
  });

  it("rechaza fila vacía, ventanilla libre y ocupada sin alterar el estado", async () => {
    const client = create();
    await sinCambios(
      client,
      () => client.asignar_siguiente({ ventanillaId: "V01" }),
      "FILA_VACIA",
      "V01",
    );
    await sinCambios(
      client,
      () => client.finalizar_atencion({ ventanillaId: "V01" }),
      "VENTANILLA_LIBRE",
      "V01",
    );
    for (const id of ["V01", "V02", "V03"] as const) {
      await client.registrar_turno();
      await client.asignar_siguiente({ ventanillaId: id });
    }
    await client.registrar_turno();
    await sinCambios(
      client,
      () => client.asignar_siguiente({ ventanillaId: "V02" }),
      "VENTANILLA_OCUPADA",
      "V02",
    );
    expect((await client.obtener_estado()).filaEspera).toHaveLength(1);
  });

  it.each(["EN_ATENCION", "ATENDIDO", "CANCELADO"])(
    "impide cancelar un turno %s",
    async (estado) => {
      const client = create();
      const turno = await client.registrar_turno();
      if (estado === "CANCELADO") await client.cancelar_turno({ turnoId: turno.id });
      else {
        await client.asignar_siguiente({ ventanillaId: "V01" });
        if (estado === "ATENDIDO") await client.finalizar_atencion({ ventanillaId: "V01" });
      }
      await sinCambios(
        client,
        () => client.cancelar_turno({ turnoId: turno.id }),
        "ESTADO_TURNO_INVALIDO",
        turno.id,
      );
    },
  );

  it("habilita y deshabilita sin permitir cero ventanillas activas ni deshabilitar ocupadas", async () => {
    const client = create();
    for (const id of ["V02", "V03"] as const)
      await client.cambiar_estado_ventanilla({ ventanillaId: id, habilitada: false });
    await sinCambios(
      client,
      () => client.cambiar_estado_ventanilla({ ventanillaId: "V01", habilitada: false }),
      "ULTIMA_VENTANILLA_ACTIVA",
      "V01",
    );
    await client.registrar_turno();
    await sinCambios(
      client,
      () => client.asignar_siguiente({ ventanillaId: "V02" }),
      "VENTANILLA_INHABILITADA",
      "V02",
    );
    await client.asignar_siguiente({ ventanillaId: "V01" });
    await sinCambios(
      client,
      () => client.cambiar_estado_ventanilla({ ventanillaId: "V01", habilitada: false }),
      "VENTANILLA_OCUPADA",
      "V01",
    );
    expect(
      await client.cambiar_estado_ventanilla({ ventanillaId: "V02", habilitada: true }),
    ).toEqual({ id: "V02", habilitada: true, turnoActualId: null });
    // Repetir el valor ya aplicado es un éxito y publica un snapshot.
    const listener = vi.fn();
    await client.escuchar_estado(listener);
    await client.cambiar_estado_ventanilla({ ventanillaId: "V03", habilitada: false });
    expect(listener).toHaveBeenCalledTimes(1);
  });

  it("normaliza IDs y valida entradas runtime sin mutaciones ni eventos", async () => {
    const client = create();
    await sinCambios(
      client,
      () => client.cancelar_turno({ turnoId: "  " }),
      "ENTRADA_INVALIDA",
      null,
    );
    await sinCambios(
      client,
      () => client.cancelar_turno({ turnoId: "T-9999" }),
      "TURNO_INEXISTENTE",
      "T-9999",
    );
    await sinCambios(
      client,
      // @ts-expect-error Comprobar una entrada inválida recibida fuera del tipado TS.
      () => client.cancelar_turno({ turnoId: 42 }),
      "ENTRADA_INVALIDA",
      null,
    );
    await sinCambios(
      client,
      // @ts-expect-error Comprobar validación del límite IPC simulado.
      () => client.asignar_siguiente({ ventanillaId: "V04" }),
      "VENTANILLA_INVALIDA",
      "V04",
    );
    // @ts-expect-error La API debe rechazar argumentos ausentes también en runtime.
    await sinCambios(client, () => client.finalizar_atencion(), "ENTRADA_INVALIDA", null);
    await sinCambios(
      client,
      // @ts-expect-error No se debe convertir un string truthy en boolean.
      () => client.cambiar_estado_ventanilla({ ventanillaId: "V01", habilitada: "false" }),
      "ENTRADA_INVALIDA",
      "V01",
    );
    const t = await client.registrar_turno();
    expect(t.id).toBe("T-0001");
    // @ts-expect-error El trim se comprueba en runtime para las entradas externas.
    expect(await client.asignar_siguiente({ ventanillaId: " V02 " })).toMatchObject({ id: t.id });
  });

  it("publica una vez por mutación y permite desuscripción independiente e idempotente", async () => {
    const client = create();
    const listener = vi.fn();
    const off1 = await client.escuchar_estado(listener);
    const off2 = await client.escuchar_estado(listener);
    expect(listener).not.toHaveBeenCalled();
    await client.obtener_estado();
    await client.abrir_ventana({ vista: "cliente" });
    await client.abrir_ventana({ vista: "cajero" });
    expect(listener).not.toHaveBeenCalled();
    const turno = await client.registrar_turno();
    expect(listener).toHaveBeenCalledTimes(2);
    off1();
    off1();
    await client.cancelar_turno({ turnoId: turno.id });
    expect(listener).toHaveBeenCalledTimes(3);
    off2();
    await client.registrar_turno();
    expect(listener).toHaveBeenCalledTimes(3);
    await sinCambios(
      client,
      // @ts-expect-error Una vista no admitida debe rechazarse, sin abrir ventanas.
      () => client.abrir_ventana({ vista: "admin" }),
      "ENTRADA_INVALIDA",
      null,
    );
    expect(EVENTO_ESTADO_ACTUALIZADO).toBe("sgt://estado-actualizado");
  });

  it("aísla retornos, snapshots, listeners y otras instancias", async () => {
    const client = create();
    const otro = create();
    expect(await otro.obtener_estado()).toEqual(vacio);
    let recibido: EstadoSistema | undefined;
    await client.escuchar_estado((estado) => {
      estado.historial[0].estado = "CANCELADO";
      estado.ventanillas[0].habilitada = false;
    });
    await client.escuchar_estado((estado) => {
      recibido = estado;
    });
    const turno = await client.registrar_turno();
    turno.estado = "ATENDIDO";
    expect(recibido?.historial[0].estado).toBe("EN_ESPERA");
    const estado = await client.obtener_estado();
    estado.historial[0].id = "roto";
    estado.ventanillas[0].habilitada = false;
    expect((await client.obtener_estado()).historial[0]).toMatchObject({
      id: "T-0001",
      estado: "EN_ESPERA",
    });
    expect((await client.obtener_estado()).ventanillas[0].habilitada).toBe(true);
    expect(await otro.obtener_estado()).toEqual(vacio);
    expect(await otro.registrar_turno()).toMatchObject({ id: "T-0001" });
  });

  it("aísla el fallo de un listener y conserva el orden con mutaciones reentrantes", async () => {
    const client = create();
    const report = vi.spyOn(console, "error").mockImplementation(() => {});
    const observados: number[] = [];
    let segundo: Promise<unknown> | undefined;
    await client.escuchar_estado(() => {
      throw new Error("consumidor roto");
    });
    await client.escuchar_estado((estado) => {
      if (estado.historial.length === 1) segundo = client.registrar_turno();
    });
    await client.escuchar_estado((estado) => {
      observados.push(estado.historial.length);
    });
    expect(await client.registrar_turno()).toMatchObject({ id: "T-0001" });
    await segundo;
    expect(observados).toEqual([1, 2]);
    expect(report).toHaveBeenCalledTimes(2);
    expect((await client.obtener_estado()).historial).toHaveLength(2);
  });

  it("un reloj inválido no consume un ID ni emite eventos", async () => {
    let fecha = new Date("inválida");
    const client = createMockSgtClient({ now: () => fecha });
    await sinCambios(client, () => client.registrar_turno(), "ENTRADA_INVALIDA", null);
    fecha = new Date("2026-11-18T01:00:00.000Z");
    expect(await client.registrar_turno()).toMatchObject({ id: "T-0001", secuencia: 1 });
  });
});
