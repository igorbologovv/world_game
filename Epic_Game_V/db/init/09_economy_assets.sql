CREATE TABLE IF NOT EXISTS asset_families (
    id BIGSERIAL PRIMARY KEY,
    code TEXT NOT NULL UNIQUE,
    name TEXT NOT NULL,
    kind TEXT NOT NULL DEFAULT 'stock'
        CHECK (kind IN ('stock', 'capacity')),
    tiered BOOLEAN NOT NULL DEFAULT false,
    tradeable BOOLEAN NOT NULL DEFAULT true,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS asset_quality_rules (
    asset_family_id BIGINT PRIMARY KEY
        REFERENCES asset_families(id) ON DELETE CASCADE,

    base_points NUMERIC NOT NULL CHECK (base_points > 0),
    growth_rate NUMERIC NOT NULL CHECK (growth_rate > 0)
);
