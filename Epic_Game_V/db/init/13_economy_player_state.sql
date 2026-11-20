CREATE TABLE IF NOT EXISTS player_economy_state (
    uid TEXT PRIMARY KEY
        REFERENCES users(uid) ON DELETE CASCADE,

    last_settled_at TIMESTAMPTZ NOT NULL DEFAULT now(),

    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS player_population (
    uid TEXT PRIMARY KEY
        REFERENCES users(uid) ON DELETE CASCADE,

    total_population INTEGER NOT NULL DEFAULT 10
        CHECK (total_population >= 0),

    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS player_asset_balances (
    uid TEXT NOT NULL
        REFERENCES users(uid) ON DELETE CASCADE,

    asset_family_id BIGINT NOT NULL
        REFERENCES asset_families(id) ON DELETE RESTRICT,

    tier INTEGER NOT NULL DEFAULT 0 CHECK (tier >= 0),

    amount NUMERIC NOT NULL DEFAULT 0 CHECK (amount >= 0),

    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),

    PRIMARY KEY (uid, asset_family_id, tier)
);

CREATE TABLE IF NOT EXISTS economy_buildings (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),

    owner_uid TEXT NOT NULL
        REFERENCES users(uid) ON DELETE CASCADE,

    map_id BIGINT NOT NULL
        REFERENCES economy_maps(id) ON DELETE RESTRICT,

    building_type_id BIGINT NOT NULL
        REFERENCES building_type_definitions(id) ON DELETE RESTRICT,

    tile_x INTEGER NOT NULL CHECK (tile_x >= 0),
    tile_y INTEGER NOT NULL CHECK (tile_y >= 0),
    tile_z INTEGER NOT NULL DEFAULT 0 CHECK (tile_z >= 0),

    footprint_width INTEGER NOT NULL CHECK (footprint_width > 0),
    footprint_height INTEGER NOT NULL CHECK (footprint_height > 0),
    footprint_depth INTEGER NOT NULL DEFAULT 1 CHECK (footprint_depth > 0),

    level INTEGER NOT NULL DEFAULT 1 CHECK (level >= 1),

    assigned_workers INTEGER NOT NULL DEFAULT 0
        CHECK (assigned_workers >= 0),

    status TEXT NOT NULL DEFAULT 'active'
        CHECK (status IN ('constructing', 'active', 'demolished')),

    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),

    CONSTRAINT economy_buildings_no_overlap
        EXCLUDE USING gist (
            map_id WITH =,
            int4range(tile_x, tile_x + footprint_width, '[)') WITH &&,
            int4range(tile_y, tile_y + footprint_height, '[)') WITH &&,
            int4range(tile_z, tile_z + footprint_depth, '[)') WITH &&
        )
        WHERE (status <> 'demolished')
);

CREATE TABLE IF NOT EXISTS player_building_cell_budgets (
    uid TEXT PRIMARY KEY
        REFERENCES users(uid) ON DELETE CASCADE,

    total_cells INTEGER NOT NULL DEFAULT 100
        CHECK (total_cells >= 0),

    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
