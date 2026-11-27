import { describe, expect, it } from "vitest";
import { esAppError, esCodigoError, normalizarError } from "./sgtErrors";

describe("sgtErrors", () => {
  it("reconoce códigos y objetos AppError válidos del contrato", () => {
    expect(esCodigoError("BASE_DATOS_CORRUPTA")).toBe(true);
    expect(esCodigoError("OTRO_CODIGO")).toBe(false);

    const errorValido = {
      codigo: "VENTANILLA_OCUPADA",
      mensaje: "La ventanilla V01 está ocupada.",
      recursoId: "V01",
    };
    expect(esAppError(errorValido)).toBe(true);
    expect(normalizarError(errorValido)).toEqual(errorValido);
  });

  it("rechaza objetos malformados y normaliza Error o valores desconocidos", () => {
    expect(esAppError(null)).toBe(false);
    expect(esAppError({ codigo: "FILA_VACIA", mensaje: "   ", recursoId: null })).toBe(false);
    expect(esAppError({ codigo: "DESCONOCIDO", mensaje: "x", recursoId: null })).toBe(false);
    expect(esAppError({ codigo: "FILA_VACIA", mensaje: "ok", recursoId: 123 })).toBe(false);

    expect(normalizarError(new Error("Fallo de disco"))).toEqual({
      codigo: "ERROR_PERSISTENCIA",
      mensaje: "Fallo de disco",
      recursoId: null,
    });

    expect(normalizarError("desconocido")).toEqual({
      codigo: "ERROR_PERSISTENCIA",
      mensaje: "Ocurrió un error inesperado al comunicarse con el sistema de turnos.",
      recursoId: null,
    });
  });
});
