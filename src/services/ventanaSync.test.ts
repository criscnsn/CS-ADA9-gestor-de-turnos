import { describe, expect, it, vi } from "vitest";
import { createMockSgtClient } from "./mockSgtClient";
import { EVENTO_ESTADO_ACTUALIZADO } from "./sgtClient";
import type { Desuscribir, EscucharEstado, SgtClient } from "./sgtClient";
import { hidratarYSincronizarVentana, resolverVistaDesdeUrl } from "./ventanaSync";
import type { AppError, EstadoSistema, TipoVista } from "../types/sgt";

function crearEstadoBase(): EstadoSistema {
  return {
    ventanillas: [
      { id: "V01", habilitada: true, turnoActualId: null },
      { id: "V02", habilitada: true, turnoActualId: null },
      { id: "V03", habilitada: true, turnoActualId: null },
    ],
    filaEspera: [],
    historial: [],
  };
}

/**
 * DOBLE DE PRUEBA — EMISOR SIMULADO DE `sgt://estado-actualizado` (`#07-A`).
 *
 * Simula exclusivamente en pruebas unitarias el canal de eventos IPC y una promesa
 * controlable de `obtener_estado()`, permitiendo verificar hidratación inicial y
 * condiciones de carrera entre múltiples ventanas sin sustituir la emisión real
 * del backend en Tauri (responsabilidad de `#06` / GH `#14`).
 */
function crearEmisorSimuladoParaPruebas(opciones?: {
  estadoInicial?: EstadoSistema;
  obtenerEstadoPersonalizado?: () => Promise<EstadoSistema>;
}) {
  let snapshotActual: EstadoSistema = structuredClone(opciones?.estadoInicial ?? crearEstadoBase());
  const suscriptores = new Set<EscucharEstado>();
  const aperturasSolicitadas: TipoVista[] = [];

  const client: SgtClient = {
    async obtener_estado() {
      if (opciones?.obtenerEstadoPersonalizado) {
        return opciones.obtenerEstadoPersonalizado();
      }
      return structuredClone(snapshotActual);
    },
    async registrar_turno() {
      throw new Error("No usado directamente en el emisor simulado puro.");
    },
    async asignar_siguiente() {
      throw new Error("No usado directamente en el emisor simulado puro.");
    },
    async finalizar_atencion() {
      throw new Error("No usado directamente en el emisor simulado puro.");
    },
    async cancelar_turno() {
      throw new Error("No usado directamente en el emisor simulado puro.");
    },
    async cambiar_estado_ventanilla() {
      throw new Error("No usado directamente en el emisor simulado puro.");
    },
    async abrir_ventana({ vista }) {
      aperturasSolicitadas.push(vista);
    },
    async escuchar_estado(listener: EscucharEstado): Promise<Desuscribir> {
      suscriptores.add(listener);
      return () => {
        suscriptores.delete(listener);
      };
    },
  };

  return {
    canalEventoSimulado: EVENTO_ESTADO_ACTUALIZADO,
    client,
    aperturasSolicitadas,
    totalSuscriptoresActivos: () => suscriptores.size,
    emitirEstadoSimulado(nuevoEstado: EstadoSistema) {
      snapshotActual = structuredClone(nuevoEstado);
      for (const listener of [...suscriptores]) {
        listener(structuredClone(snapshotActual));
      }
    },
  };
}

function crearEstadoConTurnos(idsEnEspera: string[]): EstadoSistema {
  const base = crearEstadoBase();
  base.historial = idsEnEspera.map((id, indice) => ({
    id,
    secuencia: indice + 1,
    fechaHoraRegistro: "2026-11-25T15:00:00.000Z",
    estado: "EN_ESPERA",
    ventanillaAsignada: null,
  }));
  base.filaEspera = structuredClone(base.historial);
  return base;
}

describe("sincronización multi-ventana e hidratación (#07-A)", () => {
  it("resuelve ?vista=cliente y ?vista=cajero en query params o rutas", () => {
    expect(resolverVistaDesdeUrl("?vista=cliente")).toBe("cliente");
    expect(resolverVistaDesdeUrl("?vista=cajero")).toBe("cajero");
    expect(resolverVistaDesdeUrl("index.html?vista=cliente")).toBe("cliente");
    expect(resolverVistaDesdeUrl("index.html?vista=cajero")).toBe("cajero");
    expect(resolverVistaDesdeUrl("vista=cliente&modo=dev")).toBe("cliente");

    expect(resolverVistaDesdeUrl("")).toBeNull();
    expect(resolverVistaDesdeUrl("   ")).toBeNull();
    expect(resolverVistaDesdeUrl("?vista=admin")).toBeNull();
    expect(resolverVistaDesdeUrl("?vista=CLIENTE")).toBeNull();
    expect(resolverVistaDesdeUrl("?vista=cliente&vista=cajero")).toBeNull();
    expect(resolverVistaDesdeUrl("index.html")).toBeNull();
  });

  it("hidrata múltiples ventanas y actualiza en vivo con emisor simulado", async () => {
    const estadoInicial = crearEstadoConTurnos(["T-0001"]);
    const simulador = crearEmisorSimuladoParaPruebas({ estadoInicial });
    expect(simulador.canalEventoSimulado).toBe("sgt://estado-actualizado");

    let estadoCajero: EstadoSistema | null = null;
    let estadoCliente1: EstadoSistema | null = null;
    let estadoCliente2: EstadoSistema | null = null;

    const ventanaCajero = await hidratarYSincronizarVentana(simulador.client, (estado) => {
      estadoCajero = estado;
    });
    const ventanaCliente1 = await hidratarYSincronizarVentana(simulador.client, (estado) => {
      estadoCliente1 = estado;
    });
    const ventanaCliente2 = await hidratarYSincronizarVentana(simulador.client, (estado) => {
      estadoCliente2 = estado;
    });

    expect(simulador.totalSuscriptoresActivos()).toBe(3);
    expect(ventanaCajero.estadoInicial).toEqual(estadoInicial);
    expect(estadoCajero).toEqual(estadoInicial);
    expect(estadoCliente1).toEqual(estadoInicial);
    expect(estadoCliente2).toEqual(estadoInicial);

    const estadoActualizado = crearEstadoConTurnos(["T-0001", "T-0002"]);
    simulador.emitirEstadoSimulado(estadoActualizado);

    expect(estadoCajero).toEqual(estadoActualizado);
    expect(estadoCliente1).toEqual(estadoActualizado);
    expect(estadoCliente2).toEqual(estadoActualizado);

    // Mutar el objeto recibido en una ventana no debe contaminar a las demás.
    estadoCliente1!.filaEspera[0].id = "ALTERADO_LOCALMENTE";
    expect(estadoCajero!.filaEspera[0].id).toBe("T-0001");
    expect(estadoCliente2!.filaEspera[0].id).toBe("T-0001");

    // Cerrar una ventana cliente desuscribe únicamente esa instancia (idempotente).
    ventanaCliente1.cerrar();
    ventanaCliente1.cerrar();
    expect(simulador.totalSuscriptoresActivos()).toBe(2);

    const tercerEstado = crearEstadoConTurnos(["T-0001", "T-0002", "T-0003"]);
    simulador.emitirEstadoSimulado(tercerEstado);

    expect(estadoCajero!.filaEspera).toHaveLength(3);
    expect(estadoCliente2!.filaEspera).toHaveLength(3);
    expect(estadoCliente1!.filaEspera).toHaveLength(2);

    ventanaCajero.cerrar();
    ventanaCliente2.cerrar();
    expect(simulador.totalSuscriptoresActivos()).toBe(0);
  });

  it("evita condiciones de carrera durante obtener_estado en vuelo", async () => {
    const snapshotViejo = crearEstadoConTurnos(["T-0001"]);
    const snapshotMasReciente = crearEstadoConTurnos(["T-0001", "T-0002"]);

    let resolverObtenerEstado!: (valor: EstadoSistema) => void;
    const promesaObtenerEstado = new Promise<EstadoSistema>((resolve) => {
      resolverObtenerEstado = resolve;
    });

    const simulador = crearEmisorSimuladoParaPruebas({
      estadoInicial: snapshotViejo,
      obtenerEstadoPersonalizado: () => promesaObtenerEstado,
    });

    const snapshotsObservados: string[][] = [];
    const promesaConexion = hidratarYSincronizarVentana(simulador.client, (estado) => {
      snapshotsObservados.push(estado.filaEspera.map((t) => t.id));
    });

    // Permitir que escuchar_estado complete su registro antes de resolver obtener_estado.
    await Promise.resolve();
    expect(simulador.totalSuscriptoresActivos()).toBe(1);

    // Llega un evento en tiempo real más nuevo ANTES de que resuelva obtener_estado().
    simulador.emitirEstadoSimulado(snapshotMasReciente);

    // Después resuelve obtener_estado() con la foto antigua.
    resolverObtenerEstado(snapshotViejo);

    const conexion = await promesaConexion;
    expect(conexion.estadoInicial.filaEspera.map((t) => t.id)).toEqual(["T-0001", "T-0002"]);
    expect(snapshotsObservados).toEqual([["T-0001", "T-0002"]]);

    conexion.cerrar();
  });

  it("limpia la suscripción si obtener_estado rechaza con error", async () => {
    const falloCorrupcion: AppError = {
      codigo: "BASE_DATOS_CORRUPTA",
      mensaje: "El archivo SQLite no superó integrity_check.",
      recursoId: null,
    };

    const simulador = crearEmisorSimuladoParaPruebas({
      obtenerEstadoPersonalizado: async () => {
        throw falloCorrupcion;
      },
    });

    const onEstado = vi.fn();
    await expect(hidratarYSincronizarVentana(simulador.client, onEstado)).rejects.toEqual(
      falloCorrupcion,
    );

    expect(simulador.totalSuscriptoresActivos()).toBe(0);
    simulador.emitirEstadoSimulado(crearEstadoConTurnos(["T-0001"]));
    expect(onEstado).not.toHaveBeenCalled();
  });

  it("sincroniza Cajero y dos Clientes sobre el mock compartido", async () => {
    const client = createMockSgtClient({ now: () => new Date("2026-11-25T16:00:00.000Z") });
    await client.abrir_ventana({ vista: "cajero" });
    await client.abrir_ventana({ vista: "cliente" });
    await client.abrir_ventana({ vista: "cliente" });

    const vistas: Record<"cajero" | "cliente1" | "cliente2", EstadoSistema | null> = {
      cajero: null,
      cliente1: null,
      cliente2: null,
    };

    const cajero = await hidratarYSincronizarVentana(client, (s) => {
      vistas.cajero = s;
    });
    const cliente1 = await hidratarYSincronizarVentana(client, (s) => {
      vistas.cliente1 = s;
    });
    const cliente2 = await hidratarYSincronizarVentana(client, (s) => {
      vistas.cliente2 = s;
    });

    const turno1 = await client.registrar_turno();
    const turno2 = await client.registrar_turno();
    await client.asignar_siguiente({ ventanillaId: "V01" });

    expect(vistas.cajero).toEqual(vistas.cliente1);
    expect(vistas.cajero).toEqual(vistas.cliente2);
    expect(vistas.cajero?.ventanillas[0]).toEqual({
      id: "V01",
      habilitada: true,
      turnoActualId: turno1.id,
    });
    expect(vistas.cliente2?.filaEspera.map((t) => t.id)).toEqual([turno2.id]);

    cajero.cerrar();
    cliente1.cerrar();
    cliente2.cerrar();
  });
});
