import { createContext, useContext } from "react";
import type {
  AppError,
  EstadoSistema,
  TipoVista,
  Turno,
  Ventanilla,
  VentanillaId,
} from "../types/sgt";
import type { NotificacionSgt, ResultadoOperacion } from "./sgtErrors";

export interface SgtState {
  estado: EstadoSistema | null;
  cargando: boolean;
  operando: boolean;
  notificacion: NotificacionSgt | null;
  /** Error bloqueante reservado para `BASE_DATOS_CORRUPTA` (`RD-03`). */
  errorCritico: AppError | null;
}

export interface SgtActions {
  registrarTurno: () => Promise<ResultadoOperacion<Turno>>;
  asignarSiguiente: (ventanillaId: VentanillaId) => Promise<ResultadoOperacion<Turno>>;
  finalizarAtencion: (ventanillaId: VentanillaId) => Promise<ResultadoOperacion<Turno>>;
  cancelarTurno: (turnoId: string) => Promise<ResultadoOperacion<Turno>>;
  cambiarEstadoVentanilla: (
    ventanillaId: VentanillaId,
    habilitada: boolean,
  ) => Promise<ResultadoOperacion<Ventanilla>>;
  abrirVentana: (vista: TipoVista) => Promise<ResultadoOperacion<void>>;
  recargarEstado: () => Promise<ResultadoOperacion<EstadoSistema>>;
  limpiarNotificacion: () => void;
}

export type SgtContextValue = SgtState & SgtActions;

export const SgtContext = createContext<SgtContextValue | null>(null);

/**
 * Consume el estado reactivo y las operaciones del SGT provistos por `SgtProvider`.
 */
export function useSgt(): SgtContextValue {
  const contexto = useContext(SgtContext);
  if (!contexto) {
    throw new Error("useSgt debe utilizarse dentro de un <SgtProvider>.");
  }
  return contexto;
}
