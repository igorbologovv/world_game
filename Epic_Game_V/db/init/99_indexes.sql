CREATE INDEX IF NOT EXISTS idx_player_asset_balances_uid
ON player_asset_balances(uid);

CREATE INDEX IF NOT EXISTS idx_economy_buildings_owner_uid
ON economy_buildings(owner_uid);

CREATE INDEX IF NOT EXISTS idx_economy_buildings_map_position
ON economy_buildings(map_id, tile_x, tile_y, tile_z);

CREATE INDEX IF NOT EXISTS idx_economy_buildings_type
ON economy_buildings(building_type_id);

CREATE INDEX IF NOT EXISTS idx_production_recipes_building_type
ON production_recipes(building_type_id);

CREATE INDEX IF NOT EXISTS idx_building_upgrade_cost_rules_building_type
ON building_upgrade_cost_rules(building_type_id);

CREATE UNIQUE INDEX IF NOT EXISTS idx_building_upgrade_cost_rules_unique_asset_cost
ON building_upgrade_cost_rules (
    building_type_id,
    cost_kind,
    asset_family_id,
    min_tier,
    applies_from_level
)
WHERE asset_family_id IS NOT NULL;

CREATE UNIQUE INDEX IF NOT EXISTS idx_building_upgrade_cost_rules_unique_mt_cost
ON building_upgrade_cost_rules (
    building_type_id,
    cost_kind,
    applies_from_level
)
WHERE cost_kind = 'mt_currency';
