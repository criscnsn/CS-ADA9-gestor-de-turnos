import { useState } from "react";
import { ErrorBaseDatosView } from "./components/ErrorBaseDatosView";
import { NotificacionBanner } from "./components/NotificacionBanner";
import { SelectorVista } from "./components/SelectorVista";
import { TurnosPage } from "./pages/TurnosPage";
import { VentanillaPage } from "./pages/VentanillaPage";
import type { SgtClient } from "./services/sgtClient";
import { resolverVistaDesdeUrl } from "./services/ventanaSync";
import { SgtProvider } from "./state/SgtProvider";
import { useSgt } from "./state/sgtContext";
import type { TipoVista } from "./types/sgt";
import "./styles/App.css";

export interface AppProps {
  /** Cliente inyectable para pruebas o integración Tauri (`#11`). */
  client?: SgtClient;
  /** Query string o ruta inicial para resolver `?vista=cliente` / `?vista=cajero` en pruebas. */
  initialSearch?: string;
}

interface AppShellProps {
  initialSearch: string;
}

function AppShell({ initialSearch }: AppShellProps) {
  const {
    cargando,
    operando,
    notificacion,
    errorCritico,
    abrirVentana,
    recargarEstado,
    limpiarNotificacion,
  } = useSgt();

  const [vistaActiva, setVistaActiva] = useState<TipoVista | null>(() =>
    resolverVistaDesdeUrl(initialSearch),
  );

  if (errorCritico) {
    return (
      <main className="sgt-app">
        <ErrorBaseDatosView
          error={errorCritico}
          reintentando={operando}
          onReintentar={() => {
            void recargarEstado();
          }}
        />
      </main>
    );
  }

  return (
    <div className="sgt-app">
      {vistaActiva && (
        <header className="sgt-topbar">
          <div className="sgt-topbar__marca">
            <strong>SGT</strong>
            <span>{vistaActiva === "cliente" ? "Vista Cliente" : "Vista Cajero"}</span>
          </div>
          <nav className="sgt-topbar__acciones" aria-label="Navegación de vistas">
            <button
              type="button"
              className="sgt-btn sgt-btn--secundario"
              onClick={() => setVistaActiva(null)}
            >
              Cambiar vista
            </button>
            <button
              type="button"
              className="sgt-btn sgt-btn--secundario"
              disabled={operando}
              onClick={() => {
                void abrirVentana(vistaActiva === "cliente" ? "cajero" : "cliente");
              }}
            >
              {vistaActiva === "cliente" ? "Abrir ventana Cajero" : "Abrir ventana Cliente"}
            </button>
          </nav>
        </header>
      )}

      <NotificacionBanner notificacion={notificacion} onCerrar={limpiarNotificacion} />

      <main className="sgt-contenido">
        {cargando ? (
          <p className="sgt-cargando" role="status">
            Cargando estado del sistema...
          </p>
        ) : vistaActiva === null ? (
          <SelectorVista
            operando={operando}
            onSeleccionarVista={setVistaActiva}
            onAbrirVentana={async (vista) => {
              await abrirVentana(vista);
            }}
          />
        ) : vistaActiva === "cliente" ? (
          <TurnosPage />
        ) : (
          <VentanillaPage />
        )}
      </main>
    </div>
  );
}

export function App({ client, initialSearch }: AppProps = {}) {
  const search = initialSearch ?? (typeof window !== "undefined" ? window.location.search : "");

  return (
    <SgtProvider client={client}>
      <AppShell initialSearch={search} />
    </SgtProvider>
  );
}

export default App;
