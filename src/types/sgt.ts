/** Payloads JSON compartidos con src-tauri/src/models y docs/contrato_base.md. */
export type VentanillaId = "V01" | "V02" | "V03";
export type EstadoTurno = "EN_ESPERA" | "EN_ATENCION" | "ATENDIDO" | "CANCELADO";
export type TipoVista = "cliente" | "cajero";

export interface Turno {
  id: string;
  secuencia: number;
  fechaHoraRegistro: string;
  estado: EstadoTurno;
  ventanillaAsignada: VentanillaId | null;
}

export interface Ventanilla {
  id: VentanillaId;
  habilitada: boolean;
  turnoActualId: string | null;
}

export interface EstadoSistema {
  ventanillas: [Ventanilla, Ventanilla, Ventanilla];
  filaEspera: Turno[];
  historial: Turno[];
}

export type CodigoError =
  | "VENTANILLA_INVALIDA"
  | "VENTANILLA_OCUPADA"
  | "VENTANILLA_LIBRE"
  | "VENTANILLA_INHABILITADA"
  | "ULTIMA_VENTANILLA_ACTIVA"
  | "FILA_VACIA"
  | "TURNO_INEXISTENTE"
  | "ESTADO_TURNO_INVALIDO"
  | "ENTRADA_INVALIDA"
  | "ERROR_PERSISTENCIA"
  | "BASE_DATOS_CORRUPTA";

export interface AppError {
  codigo: CodigoError;
  mensaje: string;
  recursoId: string | null;
}
