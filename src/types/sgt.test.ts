import { describe, expect, it } from "vitest";
import vacio from "../../docs/fixtures/contrato/estado-vacio.json";
import mixto from "../../docs/fixtures/contrato/estado-mixto.json";
import errores from "../../docs/fixtures/contrato/errores.json";
import enumeraciones from "../../docs/fixtures/contrato/enumeraciones.json";
import type {
  AppError,
  CodigoError,
  EstadoSistema,
  EstadoTurno,
  TipoVista,
  Turno,
  Ventanilla,
  VentanillaId,
} from "./sgt";

const estados = ["EN_ESPERA", "EN_ATENCION", "ATENDIDO", "CANCELADO"] satisfies EstadoTurno[];
const ids = ["V01", "V02", "V03"] satisfies VentanillaId[];
const vistas = ["cliente", "cajero"] satisfies TipoVista[];
const codigos = [
  "VENTANILLA_INVALIDA",
  "VENTANILLA_OCUPADA",
  "VENTANILLA_LIBRE",
  "VENTANILLA_INHABILITADA",
  "ULTIMA_VENTANILLA_ACTIVA",
  "FILA_VACIA",
  "TURNO_INEXISTENTE",
  "ESTADO_TURNO_INVALIDO",
  "ENTRADA_INVALIDA",
  "ERROR_PERSISTENCIA",
  "BASE_DATOS_CORRUPTA",
] satisfies CodigoError[];

function asegurar(condicion: unknown): asserts condicion {
  if (!condicion) throw new Error("El fixture no cumple el contrato SGT");
}
function objeto(value: unknown): asserts value is Record<string, unknown> {
  asegurar(typeof value === "object" && value !== null && !Array.isArray(value));
}
function enumeracion<T extends string>(values: T[], value: unknown): T {
  const found = values.find((item) => item === value);
  asegurar(found !== undefined);
  return found;
}
function leerTurno(value: unknown): Turno {
  objeto(value);
  asegurar(typeof value.id === "string" && typeof value.fechaHoraRegistro === "string");
  asegurar(
    typeof value.secuencia === "number" &&
      Number.isSafeInteger(value.secuencia) &&
      value.secuencia > 0,
  );
  return {
    id: value.id,
    secuencia: value.secuencia,
    fechaHoraRegistro: value.fechaHoraRegistro,
    estado: enumeracion(estados, value.estado),
    ventanillaAsignada:
      value.ventanillaAsignada === null ? null : enumeracion(ids, value.ventanillaAsignada),
  };
}
function leerVentanilla(value: unknown): Ventanilla {
  objeto(value);
  asegurar(typeof value.habilitada === "boolean");
  asegurar(value.turnoActualId === null || typeof value.turnoActualId === "string");
  return {
    id: enumeracion(ids, value.id),
    habilitada: value.habilitada,
    turnoActualId: value.turnoActualId,
  };
}
function leerEstado(value: unknown): EstadoSistema {
  objeto(value);
  asegurar(Array.isArray(value.ventanillas) && value.ventanillas.length === 3);
  asegurar(Array.isArray(value.filaEspera) && Array.isArray(value.historial));
  return {
    ventanillas: [
      leerVentanilla(value.ventanillas[0]),
      leerVentanilla(value.ventanillas[1]),
      leerVentanilla(value.ventanillas[2]),
    ],
    filaEspera: value.filaEspera.map(leerTurno),
    historial: value.historial.map(leerTurno),
  };
}
function leerError(value: unknown): AppError {
  objeto(value);
  asegurar(typeof value.mensaje === "string");
  asegurar(value.recursoId === null || typeof value.recursoId === "string");
  return {
    codigo: enumeracion(codigos, value.codigo),
    mensaje: value.mensaje,
    recursoId: value.recursoId,
  };
}

describe("fixtures JSON compartidos con Rust", () => {
  it.each([vacio, mixto])(
    "conserva exactamente campos, null y la tupla de tres ventanillas",
    (fixture) => {
      // Los imports JSON ensanchan strings y arrays: validar y construir valores
      // tipados, sin casts que prometan compatibilidad sin comprobarla.
      const estado: EstadoSistema = leerEstado(fixture);
      expect(estado).toEqual(fixture);
    },
  );
  it("incluye todos los valores exactos de las enumeraciones", () => {
    expect({
      estadosTurno: estados,
      ventanillaIds: ids,
      tiposVista: vistas,
      codigosError: codigos,
    }).toEqual(enumeraciones);
  });
  it("lee los once errores con y sin recursoId sin perder campos", () => {
    const typed: AppError[] = errores.map(leerError);
    expect(typed).toEqual(errores);
    expect(typed.map((error) => error.codigo)).toEqual(codigos);
  });
});
