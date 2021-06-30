class AddSmartIntervalsValueToBots < ActiveRecord::Migration[5.2]
  def change
    reversible do |dir|
      dir.up do
        execute <<~SQL
        UPDATE bots
        SET settings = settings || settings || jsonb_build_object('smart_intervals_value', null)

        SQL
      end
      dir.down do
        execute <<~SQL
        UPDATE bots
        SET settings = settings - 'smart_intervals_value'
        SQL
      end
    end
  end
end
