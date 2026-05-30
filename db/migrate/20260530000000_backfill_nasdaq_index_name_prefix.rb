# Existing Nasdaq index bots cached index_name = "Nasdaq 10" and have no index_name_prefix,
# so they'd keep showing the stale "Nasdaq 10" instead of the dynamic "Nasdaq {num_coins}".
# Backfill the prefix so display_index_name reflects the user's chosen count. SQLite json_*
# functions only (no model callbacks/validations).
class BackfillNasdaqIndexNamePrefix < ActiveRecord::Migration[8.1]
  def up
    execute(<<~SQL.squish)
      UPDATE bots
      SET settings = json_set(settings, '$.index_name_prefix', 'Nasdaq')
      WHERE type = 'Bots::DcaIndex'
        AND json_extract(settings, '$.index_category_id') = 'nasdaq-100'
    SQL
  end

  def down
    execute(<<~SQL.squish)
      UPDATE bots
      SET settings = json_remove(settings, '$.index_name_prefix')
      WHERE type = 'Bots::DcaIndex'
        AND json_extract(settings, '$.index_category_id') = 'nasdaq-100'
    SQL
  end
end
