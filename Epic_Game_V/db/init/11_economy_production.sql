CREATE TABLE IF NOT EXISTS production_recipes (
    id BIGSERIAL PRIMARY KEY,

    building_type_id BIGINT NOT NULL
        REFERENCES building_type_definitions(id) ON DELETE CASCADE,

    code TEXT NOT NULL UNIQUE,

    min_building_level INTEGER NOT NULL DEFAULT 1
        CHECK (min_building_level >= 1),

    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS recipe_outputs (
    recipe_id BIGINT NOT NULL
        REFERENCES production_recipes(id) ON DELETE CASCADE,

    asset_family_id BIGINT NOT NULL
        REFERENCES asset_families(id) ON DELETE RESTRICT,

    output_tier INTEGER NOT NULL DEFAULT 0 CHECK (output_tier >= 0),

    amount_per_worker_per_hour NUMERIC NOT NULL
        CHECK (amount_per_worker_per_hour >= 0),

    PRIMARY KEY (recipe_id, asset_family_id, output_tier)
);

CREATE TABLE IF NOT EXISTS recipe_exact_inputs (
    recipe_id BIGINT NOT NULL
        REFERENCES production_recipes(id) ON DELETE CASCADE,

    asset_family_id BIGINT NOT NULL
        REFERENCES asset_families(id) ON DELETE RESTRICT,

    tier INTEGER NOT NULL DEFAULT 0 CHECK (tier >= 0),

    amount_per_hour NUMERIC NOT NULL CHECK (amount_per_hour >= 0),

    PRIMARY KEY (recipe_id, asset_family_id, tier)
);

CREATE TABLE IF NOT EXISTS recipe_point_inputs (
    recipe_id BIGINT NOT NULL
        REFERENCES production_recipes(id) ON DELETE CASCADE,

    asset_family_id BIGINT NOT NULL
        REFERENCES asset_families(id) ON DELETE RESTRICT,

    required_points_per_hour NUMERIC NOT NULL
        CHECK (required_points_per_hour >= 0),

    min_tier INTEGER NOT NULL DEFAULT 1 CHECK (min_tier >= 0),

    PRIMARY KEY (recipe_id, asset_family_id)
);

CREATE TABLE IF NOT EXISTS population_consumption_rules (
    id BIGSERIAL PRIMARY KEY,

    asset_family_id BIGINT NOT NULL
        REFERENCES asset_families(id) ON DELETE RESTRICT,

    quantity_per_person_per_hour BIGINT NOT NULL
        CHECK (quantity_per_person_per_hour >= 0),

    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),

    UNIQUE (asset_family_id)
);

CREATE TABLE IF NOT EXISTS population_growth_rules (
    id SMALLINT PRIMARY KEY DEFAULT 1 CHECK (id = 1),

    mt_cost_per_person NUMERIC NOT NULL
        CHECK (mt_cost_per_person >= 0),

    food_units_per_person BIGINT NOT NULL
        CHECK (food_units_per_person >= 0),

    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
