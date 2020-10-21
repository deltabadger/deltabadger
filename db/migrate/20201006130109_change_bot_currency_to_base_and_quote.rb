class ChangeBotCurrencyToBaseAndQuote < ActiveRecord::Migration[5.2]
  def change
    reversible do |dir|
      dir.up do
        execute <<~SQL
          UPDATE bots
          SET settings =
            settings - 'currency' ||
            jsonb_build_object('quote', settings->'currency') ||
            jsonb_build_object('base', CASE
              WHEN exchange_id = kraken.id THEN 'XBT'
              ELSE 'BTC'
              END
            )
          FROM exchanges AS kraken WHERE name = 'Kraken';
        SQL
      end
      dir.down do
        execute <<~SQL
          UPDATE bots
          SET settings =
            settings - ARRAY['base', 'quote'] ||
            jsonb_build_object('currency', settings->'quote')
        SQL
      end
    end
  end
end
