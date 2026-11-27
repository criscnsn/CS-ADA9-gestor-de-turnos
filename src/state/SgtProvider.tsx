import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { ReactNode } from "react";
import { createMockSgtClient } from "../services/mockSgtClient";
import type { Desuscribir, SgtClient } from "../services/sgtClient";
import { hidratarYSincronizarVentana } from "../services/ventanaSync";
import type { AppError, EstadoSistema, TipoVista, VentanillaId } from "../types/sgt";
import { SgtContext } from "./sgtContext";
import type { SgtContextValue } from "./sgtContext";
import { normalizarError } from "./sgtErrors";
import type { NotificacionSgt, ResultadoOperacion } from "./sgtErrors";

export interface SgtProviderProps {
  /** Cliente inyectable (`createMockSgtClient` por defecto; en `#11` se inyectará el adaptador Tauri). */
  client?: SgtClient;
  children: ReactNode;
}

/**
 * Proveedor reactivo compartido para Vista Cliente y Vista Cajero.
 * Reutiliza `hidratarYSincronizarVentana` (`#7a`) para hidratar y suscribirse sin
 * condiciones de carrera (`RI-02`), mantiene el estado sin mutaciones optimistas
 * (`RF-12`, `RD-02`) y aísla `BASE_DATOS_CORRUPTA` en `errorCritico` (`RD-03`).
 */
export function SgtProvider({ client: clientProp, children }: SgtProviderProps) {
  const [defaultClient] = useState<SgtClient>(() => createMockSgtClient());
  const client = clientProp ?? defaultClient;

  const [estado, setEstado] = useState<EstadoSistema | null>(null);
  const [cargando, setCargando] = useState<boolean>(true);
  const [operacionesActivas, setOperacionesActivas] = useState<number>(0);
  const [notificacion, setNotificacion] = useState<NotificacionSgt | null>(null);
  const [errorCritico, setErrorCritico] = useState<AppError | null>(null);

  const montadoRef = useRef<boolean>(true);

  useEffect(() => {
    montadoRef.current = true;
    let activo = true;
    let cerrarConexion: Desuscribir | null = null;

    const inicializar = async () => {
      try {
        const conexion = await hidratarYSincronizarVentana(client, (snapshot) => {
          if (!activo) return;
          setEstado(snapshot);
          setErrorCritico(null);
          setCargando(false);
        });

        if (!activo) {
          conexion.cerrar();
          return;
        }
        cerrarConexion = conexion.cerrar;
      } catch (err) {
        if (!activo) return;
        const appError = normalizarError(err);
        if (appError.codigo === "BASE_DATOS_CORRUPTA") {
          setErrorCritico(appError);
        } else {
          setNotificacion({
            tipo: "error",
            codigo: appError.codigo,
            mensaje: appError.mensaje,
            recursoId: appError.recursoId,
          });
        }
      } finally {
        if (activo) {
          setCargando(false);
        }
      }
    };

    void inicializar();

    return () => {
      activo = false;
      montadoRef.current = false;
      if (cerrarConexion) {
        cerrarConexion();
      }
    };
  }, [client]);

  const registrarError = useCallback((err: unknown): AppError => {
    const appError = normalizarError(err);
    if (montadoRef.current) {
      if (appError.codigo === "BASE_DATOS_CORRUPTA") {
        setErrorCritico(appError);
      } else {
        setNotificacion({
          tipo: "error",
          codigo: appError.codigo,
          mensaje: appError.mensaje,
          recursoId: appError.recursoId,
        });
      }
    }
    return appError;
  }, []);

  const ejecutarOperacion = useCallback(
    async <T,>(
      accion: () => Promise<T>,
      crearConfirmacion?: (data: T) => { mensaje: string; recursoId: string | null },
    ): Promise<ResultadoOperacion<T>> => {
      if (montadoRef.current) {
        setOperacionesActivas((actual) => actual + 1);
      }

      try {
        const data = await accion();
        if (montadoRef.current && crearConfirmacion) {
          const confirmacion = crearConfirmacion(data);
          setNotificacion({
            tipo: "exito",
            mensaje: confirmacion.mensaje,
            recursoId: confirmacion.recursoId,
          });
        }
        return { ok: true, data };
      } catch (err) {
        const appError = registrarError(err);
        return { ok: false, error: appError };
      } finally {
        if (montadoRef.current) {
          setOperacionesActivas((actual) => Math.max(0, actual - 1));
        }
      }
    },
    [registrarError],
  );

  const registrarTurno = useCallback(
    () =>
      ejecutarOperacion(
        () => client.registrar_turno(),
        (turno) => ({
          mensaje: `Turno ${turno.id} registrado en la fila de espera.`,
          recursoId: turno.id,
        }),
      ),
    [client, ejecutarOperacion],
  );

  const asignarSiguiente = useCallback(
    (ventanillaId: VentanillaId) =>
      ejecutarOperacion(
        () => client.asignar_siguiente({ ventanillaId }),
        (turno) => ({
          mensaje: `Turno ${turno.id} asignado a la ventanilla ${ventanillaId}.`,
          recursoId: turno.id,
        }),
      ),
    [client, ejecutarOperacion],
  );

  const finalizarAtencion = useCallback(
    (ventanillaId: VentanillaId) =>
      ejecutarOperacion(
        () => client.finalizar_atencion({ ventanillaId }),
        (turno) => ({
          mensaje: `Atención del turno ${turno.id} finalizada en ${ventanillaId}.`,
          recursoId: turno.id,
        }),
      ),
    [client, ejecutarOperacion],
  );

  const cancelarTurno = useCallback(
    (turnoId: string) =>
      ejecutarOperacion(
        () => client.cancelar_turno({ turnoId }),
        (turno) => ({
          mensaje: `Turno ${turno.id} cancelado correctamente.`,
          recursoId: turno.id,
        }),
      ),
    [client, ejecutarOperacion],
  );

  const cambiarEstadoVentanilla = useCallback(
    (ventanillaId: VentanillaId, habilitada: boolean) =>
      ejecutarOperacion(
        () => client.cambiar_estado_ventanilla({ ventanillaId, habilitada }),
        (ventanilla) => ({
          mensaje: ventanilla.habilitada
            ? `Ventanilla ${ventanilla.id} habilitada.`
            : `Ventanilla ${ventanilla.id} inhabilitada.`,
          recursoId: ventanilla.id,
        }),
      ),
    [client, ejecutarOperacion],
  );

  const abrirVentana = useCallback(
    (vista: TipoVista) => ejecutarOperacion(() => client.abrir_ventana({ vista })),
    [client, ejecutarOperacion],
  );

  const recargarEstado = useCallback(
    () =>
      ejecutarOperacion(async () => {
        const snapshot = await client.obtener_estado();
        if (montadoRef.current) {
          setEstado(snapshot);
          setErrorCritico(null);
        }
        return snapshot;
      }),
    [client, ejecutarOperacion],
  );

  const limpiarNotificacion = useCallback(() => {
    setNotificacion(null);
  }, []);

  const valor = useMemo<SgtContextValue>(
    () => ({
      estado,
      cargando,
      operando: operacionesActivas > 0,
      notificacion,
      errorCritico,
      registrarTurno,
      asignarSiguiente,
      finalizarAtencion,
      cancelarTurno,
      cambiarEstadoVentanilla,
      abrirVentana,
      recargarEstado,
      limpiarNotificacion,
    }),
    [
      estado,
      cargando,
      operacionesActivas,
      notificacion,
      errorCritico,
      registrarTurno,
      asignarSiguiente,
      finalizarAtencion,
      cancelarTurno,
      cambiarEstadoVentanilla,
      abrirVentana,
      recargarEstado,
      limpiarNotificacion,
    ],
  );

  return <SgtContext.Provider value={valor}>{children}</SgtContext.Provider>;
}
