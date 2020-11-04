class AddForceSmartIntervalToExistingBots < ActiveRecord::Migration[5.2]
  def change
    reversible do |dir|
      dir.up do
        execute <<~SQL
          UPDATE bots
          SET settings = settings || jsonb_build_object('force_smart_intervals', false)
          WHERE settings->'force_smart_intervals' IS NULL
        SQL
      end
      dir.down do
        execute <<~SQL
          UPDATE bots
          SET settings = settings - 'force_smart_intervals'
          WHERE settings->'force_smart_intervals' IS NOT NULL
        SQL
      end
    end
  end
end
