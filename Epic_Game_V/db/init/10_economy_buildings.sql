CREATE EXTENSION IF NOT EXISTS btree_gist;

CREATE TABLE IF NOT EXISTS economy_maps (
    id BIGSERIAL PRIMARY KEY,
    code TEXT NOT NULL UNIQUE,
    name TEXT NOT NULL,

    width INTEGER NOT NULL CHECK (width > 0),
    height INTEGER NOT NULL CHECK (height > 0),
    depth INTEGER NOT NULL DEFAULT 1 CHECK (depth > 0),

    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS building_type_definitions (
    id BIGSERIAL PRIMARY KEY,
    code TEXT NOT NULL UNIQUE,
    name TEXT NOT NULL,
    footprint_width INTEGER NOT NULL DEFAULT 1 CHECK (footprint_width > 0),
    footprint_height INTEGER NOT NULL DEFAULT 1 CHECK (footprint_height > 0),
    footprint_depth INTEGER NOT NULL DEFAULT 1 CHECK (footprint_depth > 0),
    placement_cost_mt NUMERIC NOT NULL DEFAULT 0 CHECK (placement_cost_mt >= 0),
    max_workers INTEGER NOT NULL CHECK (max_workers >= 0),
    max_level INTEGER NOT NULL DEFAULT 500 CHECK (max_level >= 1),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS building_progression_rules (
    id BIGSERIAL PRIMARY KEY,

    building_type_id BIGINT NOT NULL
        REFERENCES building_type_definitions(id) ON DELETE CASCADE,

    stat_code TEXT NOT NULL,

    curve_type TEXT NOT NULL
        CHECK (curve_type IN ('exponential', 'step', 'linear')),

    base_value NUMERIC NOT NULL CHECK (base_value >= 0),
    growth_rate NUMERIC CHECK (growth_rate > 0),
    step_every INTEGER CHECK (step_every > 0),
    step_amount NUMERIC CHECK (step_amount >= 0),

    UNIQUE (building_type_id, stat_code),

    CHECK (
        (
            curve_type = 'exponential'
            AND growth_rate IS NOT NULL
        )
        OR
        (
            curve_type = 'step'
            AND step_every IS NOT NULL
            AND step_amount IS NOT NULL
        )
        OR
        (
            curve_type = 'linear'
            AND step_amount IS NOT NULL
        )
    )
);
