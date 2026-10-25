-- D-STRAY-FLAGS-MIGRATION (PLAN-conditions-every-bot, CE-0): Rails' DcaIndex and Signal never read the trigger rules' switches
-- or the spending cap, so a value stored there was inert. This engine runs them on every kind, so each such switch an index or
-- signal bot holds is turned off once, before the first takeover: only keys that exist, any value but false (a malformed one
-- included) becomes false. updated_at and every other key stay as they are.
UPDATE bots SET settings = json_replace(settings,
    '$.price_limited', json('false'), '$.price_drop_limited', json('false'),
    '$.moving_average_limited', json('false'), '$.indicator_limited', json('false'),
    '$.sell_price_limited', json('false'), '$.sell_price_drop_limited', json('false'),
    '$.sell_moving_average_limited', json('false'), '$.sell_indicator_limited', json('false'),
    '$.quote_amount_limited', json('false'))
WHERE type IN ('Bots::DcaIndex', 'Bots::Signal') AND json_valid(settings) AND json_type(settings) = 'object'
  AND EXISTS (SELECT 1 FROM json_each(bots.settings) WHERE json_each.type <> 'false' AND json_each.key IN (
    'price_limited', 'price_drop_limited', 'moving_average_limited', 'indicator_limited', 'sell_price_limited',
    'sell_price_drop_limited', 'sell_moving_average_limited', 'sell_indicator_limited', 'quote_amount_limited'));
