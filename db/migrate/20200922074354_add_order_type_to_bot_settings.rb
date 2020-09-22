class AddOrderTypeToBotSettings < ActiveRecord::Migration[5.2]
  def change
    reversible do |dir|
      dir.up do
        execute <<~SQL
          UPDATE bots
          SET settings = settings || '{"order_type": "market"}'
          WHERE settings->'order_type' IS NULL
        SQL
      end
      dir.down do
        execute <<~SQL
          UPDATE bots
          SET settings = settings - 'order_type'
          WHERE settings->'order_type' IS NOT NULL
        SQL
      end
    end
  end
end
