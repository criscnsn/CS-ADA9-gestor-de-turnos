-- Migración 001: Esquema inicial del Sistema de Gestión de Turnos (SGT)
-- Motor: SQLite 3 (embebido vía rusqlite con feature "bundled")
-- Nota: Ejecutar PRAGMA foreign_keys = ON al abrir cada conexión.

PRAGMA foreign_keys = ON;

BEGIN;

-- ---------------------------------------------------------------------------
-- 1. Control de migraciones aplicadas (permite evolucionar el esquema sin
--    reconstruir tablas manualmente; ej. 002_agregar_ventanilla.sql)
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS schema_migrations (
    version     INTEGER PRIMARY KEY CHECK (version > 0),
    nombre      TEXT    NOT NULL UNIQUE CHECK (length(trim(nombre)) > 0),
    aplicada_en TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);

-- ---------------------------------------------------------------------------
-- 2. Secuencias persistentes (RN-02, CV-08)
--    Evita reutilizar identificadores o secuencias después de reiniciar.
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS secuencias (
    clave        TEXT    PRIMARY KEY CHECK (length(trim(clave)) > 0),
    ultimo_valor INTEGER NOT NULL DEFAULT 0 CHECK (ultimo_valor >= 0)
);

-- ---------------------------------------------------------------------------
-- 3. Catálogo de ventanillas (RN-03, RC-03, RN-04, RN-05, RF-13)
--    Diseño extensible: no se hardcodea una lista fija de IDs en el DDL;
--    se valida el formato ('V01', 'V02', 'V03', ..., 'V04') y se insertan
--    las 3 ventanillas predefinidas como datos iniciales.
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS ventanillas (
    id              TEXT    PRIMARY KEY CHECK (
                                length(id) >= 3
                                AND substr(id, 1, 1) = 'V'
                                AND substr(id, 2) NOT GLOB '*[^0-9]*'
                            ),
    habilitada      INTEGER NOT NULL DEFAULT 1 CHECK (habilitada IN (0, 1)),
    -- UNIQUE permite múltiples NULL (libres) pero impide asignar un mismo
    -- turno a dos ventanillas distintas al mismo tiempo (RN-05).
    turno_actual_id TEXT    NULL UNIQUE,

    -- RF-13: una ventanilla inhabilitada no puede tener un turno en atención.
    CHECK (habilitada = 1 OR turno_actual_id IS NULL),

    FOREIGN KEY (turno_actual_id) REFERENCES turnos(id) ON DELETE RESTRICT
);

-- ---------------------------------------------------------------------------
-- 4. Turnos: cola única FIFO e historial permanente (RF-02, RF-03, RF-10,
--    RN-01..RN-09, CV-01..CV-10)
--    ventanilla_asignada se valida únicamente contra ventanillas(id) por FK,
--    de modo que agregar una futura V04 no requiere alterar esta tabla.
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS turnos (
    id                  TEXT    PRIMARY KEY CHECK (length(trim(id)) > 0),
    secuencia           INTEGER NOT NULL UNIQUE CHECK (secuencia > 0),
    fecha_hora_registro TEXT    NOT NULL CHECK (length(trim(fecha_hora_registro)) > 0),
    estado              TEXT    NOT NULL CHECK (
                                    estado IN ('EN_ESPERA', 'EN_ATENCION', 'ATENDIDO', 'CANCELADO')
                                ),
    ventanilla_asignada TEXT    NULL,

    -- Coherencia estado <-> ventanilla:
    -- * EN_ESPERA y CANCELADO (solo desde espera, RN-07) nunca tienen ventanilla.
    -- * EN_ATENCION y ATENDIDO siempre conservan la ventanilla asignada (RF-10).
    CHECK (
        (estado IN ('EN_ESPERA', 'CANCELADO') AND ventanilla_asignada IS NULL)
        OR
        (estado IN ('EN_ATENCION', 'ATENDIDO') AND ventanilla_asignada IS NOT NULL)
    ),

    FOREIGN KEY (ventanilla_asignada) REFERENCES ventanillas(id) ON DELETE RESTRICT
);

-- ---------------------------------------------------------------------------
-- 5. Índices de integridad y rendimiento (RN-02, RN-04, RF-03, RF-10, RNF-P01)
-- ---------------------------------------------------------------------------

-- RN-04: como máximo 1 turno EN_ATENCION por ventanilla (escala a N ventanillas).
CREATE UNIQUE INDEX IF NOT EXISTS ux_turnos_un_activo_por_ventanilla
    ON turnos (ventanilla_asignada)
    WHERE estado = 'EN_ATENCION';

-- RF-03 / RF-04 / RN-02: cola FIFO ordenada ascendentemente por secuencia.
CREATE INDEX IF NOT EXISTS ix_turnos_cola_fifo
    ON turnos (secuencia ASC)
    WHERE estado = 'EN_ESPERA';

-- RF-10: consultas de historial por estado y orden de llegada.
CREATE INDEX IF NOT EXISTS ix_turnos_estado_secuencia
    ON turnos (estado, secuencia ASC);

-- ---------------------------------------------------------------------------
-- 6. Triggers de integridad estricta (RC-03, RN-07, RN-09, RF-13)
-- ---------------------------------------------------------------------------

-- RF-13: siempre debe quedar al menos 1 ventanilla habilitada (funciona con 3 o N cajas).
CREATE TRIGGER IF NOT EXISTS trg_ventanillas_minimo_una_activa
BEFORE UPDATE OF habilitada ON ventanillas
WHEN NEW.habilitada = 0 AND OLD.habilitada = 1
BEGIN
    SELECT CASE
        WHEN (SELECT COUNT(*) FROM ventanillas WHERE habilitada = 1 AND id != OLD.id) = 0
        THEN RAISE(ABORT, 'ULTIMA_VENTANILLA_ACTIVA')
    END;
END;

-- RC-03 / RN-03: no se permite eliminar ventanillas registradas.
CREATE TRIGGER IF NOT EXISTS trg_ventanillas_no_delete
BEFORE DELETE ON ventanillas
BEGIN
    SELECT RAISE(ABORT, 'VENTANILLAS_NO_ELIMINABLES');
END;

-- RN-09: los turnos nunca se eliminan del historial.
CREATE TRIGGER IF NOT EXISTS trg_turnos_no_delete
BEFORE DELETE ON turnos
BEGIN
    SELECT RAISE(ABORT, 'HISTORIAL_NO_ELIMINABLE');
END;

-- RN-07 / RN-09: ATENDIDO y CANCELADO son estados terminales inmutables.
CREATE TRIGGER IF NOT EXISTS trg_turnos_estados_terminales
BEFORE UPDATE ON turnos
WHEN OLD.estado IN ('ATENDIDO', 'CANCELADO')
BEGIN
    SELECT RAISE(ABORT, 'ESTADO_TERMINAL_INMUTABLE');
END;

-- ---------------------------------------------------------------------------
-- 7. Datos semilla iniciales (se insertan después de crear todas las tablas
--    para satisfacer las llaves foráneas con PRAGMA foreign_keys = ON)
-- ---------------------------------------------------------------------------
INSERT INTO secuencias (clave, ultimo_valor)
VALUES ('turno', 0)
ON CONFLICT(clave) DO NOTHING;

INSERT INTO ventanillas (id, habilitada, turno_actual_id) VALUES
    ('V01', 1, NULL),
    ('V02', 1, NULL),
    ('V03', 1, NULL)
ON CONFLICT(id) DO NOTHING;

INSERT INTO schema_migrations (version, nombre)
VALUES (1, '001_initial')
ON CONFLICT(version) DO NOTHING;

COMMIT;
