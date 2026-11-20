CREATE OR REPLACE FUNCTION settle_player_economy(p_uid TEXT)
RETURNS TABLE (
    uid TEXT,
    settled_hours BIGINT,
    previous_settled_at TIMESTAMPTZ,
    new_settled_at TIMESTAMPTZ
)
LANGUAGE plpgsql
AS $$
DECLARE
    v_previous_settled_at TIMESTAMPTZ;
    v_new_settled_at TIMESTAMPTZ;
    v_full_hours BIGINT;
BEGIN
    SELECT pes.last_settled_at
    INTO v_previous_settled_at
    FROM player_economy_state pes
    WHERE pes.uid = p_uid
    FOR UPDATE;

    IF NOT FOUND THEN
        RAISE EXCEPTION 'player_economy_state row not found for uid %', p_uid
            USING ERRCODE = 'no_data_found';
    END IF;

    v_full_hours := floor(
        extract(epoch FROM (now() - v_previous_settled_at)) / 3600
    )::BIGINT;

    IF v_full_hours <= 0 THEN
        RETURN QUERY
        SELECT p_uid, 0::BIGINT, v_previous_settled_at, v_previous_settled_at;
        RETURN;
    END IF;

    v_new_settled_at := v_previous_settled_at
        + (v_full_hours * interval '1 hour');

    WITH production_deltas AS (
        SELECT
            ro.asset_family_id,
            ro.output_tier AS tier,
            SUM(
                eb.assigned_workers::NUMERIC
                * ro.amount_per_worker_per_hour
                * COALESCE(
                    CASE bpr.curve_type
                        WHEN 'exponential' THEN
                            bpr.base_value
                            * power(
                                bpr.growth_rate,
                                GREATEST(eb.level - 1, 0)
                            )
                        WHEN 'linear' THEN
                            bpr.base_value
                            + (
                                GREATEST(eb.level - 1, 0)::NUMERIC
                                * bpr.step_amount
                            )
                        WHEN 'step' THEN
                            bpr.base_value
                            + (
                                floor(
                                    GREATEST(eb.level - 1, 0)::NUMERIC
                                    / bpr.step_every
                                )
                                * bpr.step_amount
                            )
                    END,
                    1
                )
                * v_full_hours::NUMERIC
            ) AS amount_delta
        FROM economy_buildings eb
        JOIN production_recipes pr
            ON pr.building_type_id = eb.building_type_id
        JOIN recipe_outputs ro
            ON ro.recipe_id = pr.id
        LEFT JOIN building_progression_rules bpr
            ON bpr.building_type_id = eb.building_type_id
           AND bpr.stat_code = 'efficiency'
        WHERE eb.owner_uid = p_uid
          AND eb.status = 'active'
          AND eb.assigned_workers > 0
          AND eb.level >= pr.min_building_level
        GROUP BY ro.asset_family_id, ro.output_tier
    )
    INSERT INTO player_asset_balances (
        uid,
        asset_family_id,
        tier,
        amount,
        updated_at
    )
    SELECT
        p_uid,
        pd.asset_family_id,
        pd.tier,
        pd.amount_delta,
        now()
    FROM production_deltas pd
    WHERE pd.amount_delta > 0
    ON CONFLICT ON CONSTRAINT player_asset_balances_pkey DO UPDATE
    SET amount = player_asset_balances.amount + EXCLUDED.amount,
        updated_at = now();

    WITH consumption_deltas AS (
        SELECT
            pcr.asset_family_id,
            0 AS tier,
            (
                COALESCE(pp.total_population, 0)::NUMERIC
                * pcr.quantity_per_person_per_hour::NUMERIC
                * v_full_hours::NUMERIC
            ) AS amount_delta
        FROM population_consumption_rules pcr
        LEFT JOIN player_population pp
            ON pp.uid = p_uid
    ),
    updated_balances AS (
        UPDATE player_asset_balances pab
        SET amount = GREATEST(pab.amount - cd.amount_delta, 0),
            updated_at = now()
        FROM consumption_deltas cd
        WHERE pab.uid = p_uid
          AND pab.asset_family_id = cd.asset_family_id
          AND pab.tier = cd.tier
          AND cd.amount_delta > 0
        RETURNING pab.asset_family_id, pab.tier
    )
    INSERT INTO player_asset_balances (
        uid,
        asset_family_id,
        tier,
        amount,
        updated_at
    )
    SELECT
        p_uid,
        cd.asset_family_id,
        cd.tier,
        0,
        now()
    FROM consumption_deltas cd
    WHERE cd.amount_delta > 0
      AND NOT EXISTS (
          SELECT 1
          FROM updated_balances ub
          WHERE ub.asset_family_id = cd.asset_family_id
            AND ub.tier = cd.tier
      );

    UPDATE player_economy_state pes
    SET last_settled_at = v_new_settled_at,
        updated_at = now()
    WHERE pes.uid = p_uid;

    RETURN QUERY
    SELECT p_uid, v_full_hours, v_previous_settled_at, v_new_settled_at;
END;
$$;
