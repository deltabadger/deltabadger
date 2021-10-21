SELECT  bot_id,
        sum (amount * rate) as total_cost,
        sum (amount) as total_amount,
        exchange_id,
        settings,
        bots.created_at
FROM "bots"
         INNER JOIN "transactions" ON "transactions"."bot_id" = "bots"."id"
WHERE (settings->>'type' = 'buy') GROUP BY "bot_id", "bots"."exchange_id", "bots"."settings", "bots"."created_at"