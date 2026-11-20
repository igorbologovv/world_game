INSERT INTO asset_families (code, name, kind, tiered, tradeable)
VALUES
    ('food', 'Food', 'stock', false, true),
    ('metal', 'Metal', 'stock', true, true),
    ('tool', 'Tool', 'stock', true, true)
ON CONFLICT (code) DO NOTHING;

INSERT INTO asset_quality_rules (asset_family_id, base_points, growth_rate)
SELECT id, 100, 1.18
FROM asset_families
WHERE code = 'metal'
ON CONFLICT (asset_family_id) DO NOTHING;

INSERT INTO asset_quality_rules (asset_family_id, base_points, growth_rate)
SELECT id, 100, 1.15
FROM asset_families
WHERE code = 'tool'
ON CONFLICT (asset_family_id) DO NOTHING;

INSERT INTO economy_maps (code, name, width, height, depth)
VALUES
    ('main_world', 'Main World', 1000000, 1000000, 1)
ON CONFLICT (code) DO NOTHING;

INSERT INTO building_type_definitions (
    code,
    name,
    footprint_width,
    footprint_height,
    footprint_depth,
    placement_cost_mt,
    max_workers,
    max_level
)
VALUES
    ('farm', 'Farm', 5, 5, 1, 500, 10, 500)
ON CONFLICT (code) DO UPDATE
SET placement_cost_mt = EXCLUDED.placement_cost_mt,
    max_workers = EXCLUDED.max_workers;

INSERT INTO building_progression_rules (
    building_type_id,
    stat_code,
    curve_type,
    base_value,
    growth_rate,
    step_every,
    step_amount
)
SELECT id, 'efficiency', 'exponential', 1.00, 1.12, NULL, NULL
FROM building_type_definitions
WHERE code = 'farm'
ON CONFLICT (building_type_id, stat_code) DO NOTHING;

INSERT INTO production_recipes (building_type_id, code, min_building_level)
SELECT id, 'farm_food_basic', 1
FROM building_type_definitions
WHERE code = 'farm'
ON CONFLICT (code) DO NOTHING;

INSERT INTO recipe_outputs (
    recipe_id,
    asset_family_id,
    output_tier,
    amount_per_worker_per_hour
)
SELECT
    pr.id,
    af.id,
    0,
    130
FROM production_recipes pr
JOIN asset_families af ON af.code = 'food'
WHERE pr.code = 'farm_food_basic'
ON CONFLICT (recipe_id, asset_family_id, output_tier) DO UPDATE
SET amount_per_worker_per_hour = EXCLUDED.amount_per_worker_per_hour;

INSERT INTO population_consumption_rules (
    asset_family_id,
    quantity_per_person_per_hour
)
SELECT id, 100
FROM asset_families
WHERE code = 'food'
ON CONFLICT (asset_family_id) DO NOTHING;

INSERT INTO population_growth_rules (
    id,
    mt_cost_per_person,
    food_units_per_person
)
VALUES (1, 100, 100)
ON CONFLICT (id) DO UPDATE
SET mt_cost_per_person = EXCLUDED.mt_cost_per_person,
    food_units_per_person = EXCLUDED.food_units_per_person;

INSERT INTO building_upgrade_cost_rules (
    building_type_id,
    cost_kind,
    asset_family_id,
    min_tier,
    base_amount,
    growth_rate,
    applies_from_level
)
SELECT
    btd.id,
    'exact_asset',
    af.id,
    0,
    100,
    1.25,
    1
FROM building_type_definitions btd
JOIN asset_families af ON af.code = 'food'
WHERE btd.code = 'farm'
  AND NOT EXISTS (
      SELECT 1
      FROM building_upgrade_cost_rules r
      WHERE r.building_type_id = btd.id
        AND r.cost_kind = 'exact_asset'
        AND r.asset_family_id = af.id
        AND r.min_tier = 0
        AND r.applies_from_level = 1
  );

INSERT INTO building_upgrade_cost_rules (
    building_type_id,
    cost_kind,
    asset_family_id,
    min_tier,
    base_amount,
    growth_rate,
    applies_from_level
)
SELECT
    btd.id,
    'category_points',
    af.id,
    1,
    10000,
    1.35,
    5
FROM building_type_definitions btd
JOIN asset_families af ON af.code = 'metal'
WHERE btd.code = 'farm'
  AND NOT EXISTS (
      SELECT 1
      FROM building_upgrade_cost_rules r
      WHERE r.building_type_id = btd.id
        AND r.cost_kind = 'category_points'
        AND r.asset_family_id = af.id
        AND r.min_tier = 1
        AND r.applies_from_level = 5
  );

INSERT INTO building_upgrade_cost_rules (
    building_type_id,
    cost_kind,
    asset_family_id,
    min_tier,
    base_amount,
    growth_rate,
    applies_from_level
)
SELECT
    btd.id,
    'category_points',
    af.id,
    1,
    5000,
    1.40,
    10
FROM building_type_definitions btd
JOIN asset_families af ON af.code = 'tool'
WHERE btd.code = 'farm'
  AND NOT EXISTS (
      SELECT 1
      FROM building_upgrade_cost_rules r
      WHERE r.building_type_id = btd.id
        AND r.cost_kind = 'category_points'
        AND r.asset_family_id = af.id
        AND r.min_tier = 1
        AND r.applies_from_level = 10
  );
