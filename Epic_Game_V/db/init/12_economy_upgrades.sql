CREATE TABLE IF NOT EXISTS building_upgrade_cost_rules (
    id BIGSERIAL PRIMARY KEY,

    building_type_id BIGINT NOT NULL
        REFERENCES building_type_definitions(id) ON DELETE CASCADE,

    cost_kind TEXT NOT NULL
        CHECK (cost_kind IN ('exact_asset', 'category_points', 'mt_currency')),

    asset_family_id BIGINT
        REFERENCES asset_families(id) ON DELETE RESTRICT,

    min_tier INTEGER CHECK (min_tier >= 0),

    base_amount NUMERIC NOT NULL CHECK (base_amount > 0),
    growth_rate NUMERIC NOT NULL CHECK (growth_rate > 0),

    applies_from_level INTEGER NOT NULL DEFAULT 1
        CHECK (applies_from_level >= 1),

    CHECK (
        (
            cost_kind = 'mt_currency'
            AND asset_family_id IS NULL
            AND min_tier IS NULL
        )
        OR
        (
            cost_kind IN ('exact_asset', 'category_points')
            AND asset_family_id IS NOT NULL
            AND min_tier IS NOT NULL
        )
    )
);
