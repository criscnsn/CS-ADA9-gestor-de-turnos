import type { EstadoSistema, TipoVista, Turno, Ventanilla, VentanillaId } from "../types/sgt";

export const EVENTO_ESTADO_ACTUALIZADO = "sgt://estado-actualizado";
export type Desuscribir = () => void;
/** El retorno se ignora; el mock también aísla rechazos de callbacks async. */
export type EscucharEstado = (estado: EstadoSistema) => void;

/**
 * Puerto asíncrono común para mock y futuro adaptador Tauri.
 * Los rechazos funcionales son objetos AppError del contrato.
 */
export interface SgtClient {
  obtener_estado(): Promise<EstadoSistema>;
  registrar_turno(): Promise<Turno>;
  asignar_siguiente(args: { ventanillaId: VentanillaId }): Promise<Turno>;
  finalizar_atencion(args: { ventanillaId: VentanillaId }): Promise<Turno>;
  cancelar_turno(args: { turnoId: string }): Promise<Turno>;
  cambiar_estado_ventanilla(args: {
    ventanillaId: VentanillaId;
    habilitada: boolean;
  }): Promise<Ventanilla>;
  /** En el mock valida la vista; no abre ninguna ventana. */
  abrir_ventana(args: { vista: TipoVista }): Promise<void>;
  /** No emite estado inicial. Desuscribir es idempotente. */
  escuchar_estado(listener: EscucharEstado): Promise<Desuscribir>;
}
