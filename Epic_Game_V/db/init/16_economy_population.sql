CREATE OR REPLACE FUNCTION increase_player_population(
    p_uid TEXT,
    p_amount INTEGER
)
RETURNS TABLE (
    uid TEXT,
    total_population INTEGER,
    added_population INTEGER,
    mt_cost NUMERIC,
    food_cost NUMERIC,
    remaining_mt_balance NUMERIC,
    remaining_food_balance NUMERIC
)
LANGUAGE plpgsql
AS $$
DECLARE
    v_current_population INTEGER;
    v_mt_balance NUMERIC;
    v_food_balance NUMERIC;
    v_food_asset_family_id BIGINT;
    v_mt_cost_per_person NUMERIC;
    v_food_units_per_person BIGINT;
    v_total_mt_cost NUMERIC;
    v_total_food_cost NUMERIC;
BEGIN
    IF p_amount IS NULL OR p_amount <= 0 THEN
        RAISE EXCEPTION 'population increase amount must be positive'
            USING ERRCODE = '22023';
    END IF;

    PERFORM 1 FROM settle_player_economy(p_uid);

    SELECT mt_cost_per_person, food_units_per_person
    INTO v_mt_cost_per_person, v_food_units_per_person
    FROM population_growth_rules
    WHERE id = 1;

    IF NOT FOUND THEN
        RAISE EXCEPTION 'population growth rule is missing';
    END IF;

    SELECT id
    INTO v_food_asset_family_id
    FROM asset_families
    WHERE code = 'food';

    IF NOT FOUND THEN
        RAISE EXCEPTION 'food asset family is missing';
    END IF;

    SELECT u.balance
    INTO v_mt_balance
    FROM users u
    WHERE u.uid = p_uid
    FOR UPDATE;

    IF NOT FOUND THEN
        RAISE EXCEPTION 'user not found for uid %', p_uid
            USING ERRCODE = '23503';
    END IF;

    SELECT pp.total_population
    INTO v_current_population
    FROM player_population pp
    WHERE pp.uid = p_uid
    FOR UPDATE;

    IF NOT FOUND THEN
        RAISE EXCEPTION 'player_population row not found for uid %', p_uid
            USING ERRCODE = '23503';
    END IF;

    INSERT INTO player_asset_balances (
        uid,
        asset_family_id,
        tier,
        amount
    )
    VALUES (
        p_uid,
        v_food_asset_family_id,
        0,
        0
    )
    ON CONFLICT ON CONSTRAINT player_asset_balances_pkey DO NOTHING;

    SELECT pab.amount
    INTO v_food_balance
    FROM player_asset_balances pab
    WHERE pab.uid = p_uid
      AND pab.asset_family_id = v_food_asset_family_id
      AND pab.tier = 0
    FOR UPDATE;

    v_total_mt_cost := v_mt_cost_per_person * p_amount;
    v_total_food_cost := v_food_units_per_person::NUMERIC * p_amount;

    IF v_mt_balance < v_total_mt_cost THEN
        RAISE EXCEPTION 'not enough MT to increase population'
            USING ERRCODE = 'P1002';
    END IF;

    IF v_food_balance < v_total_food_cost THEN
        RAISE EXCEPTION 'not enough food to increase population'
            USING ERRCODE = 'P1003';
    END IF;

    UPDATE users u
    SET balance = u.balance - v_total_mt_cost
    WHERE u.uid = p_uid
    RETURNING u.balance INTO v_mt_balance;

    UPDATE player_asset_balances pab
    SET amount = pab.amount - v_total_food_cost,
        updated_at = now()
    WHERE pab.uid = p_uid
      AND pab.asset_family_id = v_food_asset_family_id
      AND pab.tier = 0
    RETURNING pab.amount INTO v_food_balance;

    UPDATE player_population pp
    SET total_population = pp.total_population + p_amount,
        updated_at = now()
    WHERE pp.uid = p_uid
    RETURNING pp.total_population INTO v_current_population;

    RETURN QUERY
    SELECT
        p_uid,
        v_current_population,
        p_amount,
        v_total_mt_cost,
        v_total_food_cost,
        v_mt_balance,
        v_food_balance;
END;
$$;
