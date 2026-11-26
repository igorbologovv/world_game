use sqlx::PgPool;

/// Apply additive columns to preexisting PostgreSQL volumes without touching balances.
pub async fn migrate(db: &PgPool) -> Result<(), sqlx::Error> {
    for statement in [
        "ALTER TABLE economy_buildings ADD COLUMN IF NOT EXISTS planet_x DOUBLE PRECISION",
        "ALTER TABLE economy_buildings ADD COLUMN IF NOT EXISTS planet_y DOUBLE PRECISION",
        "ALTER TABLE economy_buildings ADD COLUMN IF NOT EXISTS planet_z DOUBLE PRECISION",
        "ALTER TABLE economy_buildings ADD COLUMN IF NOT EXISTS pad_elevation DOUBLE PRECISION",
        "ALTER TABLE economy_buildings ADD COLUMN IF NOT EXISTS earthwork_m3 DOUBLE PRECISION",
        "ALTER TABLE economy_buildings ADD COLUMN IF NOT EXISTS clearance_m2 DOUBLE PRECISION",
        "ALTER TABLE economy_buildings ADD COLUMN IF NOT EXISTS construction_cost NUMERIC",
        "ALTER TABLE economy_maps ADD COLUMN IF NOT EXISTS origin_x DOUBLE PRECISION",
        "ALTER TABLE economy_maps ADD COLUMN IF NOT EXISTS origin_y DOUBLE PRECISION",
        "ALTER TABLE economy_maps ADD COLUMN IF NOT EXISTS origin_z DOUBLE PRECISION",
        "ALTER TABLE economy_maps ADD COLUMN IF NOT EXISTS origin_cell INTEGER NOT NULL DEFAULT 500000",
        "ALTER TABLE economy_maps ADD COLUMN IF NOT EXISTS clearing_radius_m DOUBLE PRECISION",
    ] {
        sqlx::query(statement).execute(db).await?;
    }
    sqlx::query("UPDATE economy_maps SET origin_x = -0.97684968336687, origin_y = 0.21339114535822, origin_z = 0.01512994344821 WHERE code = 'main_world' AND origin_x IS NULL")
        .execute(db).await?;
    sqlx::query("UPDATE economy_maps SET clearing_radius_m = 55 WHERE code = 'main_world' AND clearing_radius_m IS NULL")
        .execute(db).await?;
    sqlx::query("DO $$ BEGIN IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'economy_buildings_graded_no_overlap') THEN ALTER TABLE economy_buildings ADD CONSTRAINT economy_buildings_graded_no_overlap EXCLUDE USING gist (map_id WITH =, int4range(tile_x - 3, tile_x + footprint_width + 3, '[)') WITH &&, int4range(tile_y - 3, tile_y + footprint_height + 3, '[)') WITH &&, int4range(tile_z, tile_z + footprint_depth, '[)') WITH &&) WHERE (status <> 'demolished' AND planet_x IS NOT NULL); END IF; END $$")
        .execute(db).await?;
    Ok(())
}
