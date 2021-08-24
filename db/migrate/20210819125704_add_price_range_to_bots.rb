class AddPriceRangeToBots < ActiveRecord::Migration[5.2]
  def change
    reversible do |dir|
      dir.up do
        execute <<~SQL
        UPDATE bots
        SET settings = settings || jsonb_build_object('price_range_enabled', false,
                                                      'price_range', jsonb_build_array(0, 0))

        SQL
      end
      dir.down do
        execute <<~SQL
        UPDATE bots
        SET settings = settings - 'price_range_enabled'
        SQL
        execute <<~SQL
        UPDATE bots
        SET settings = settings - 'price_range'
        SQL
      end
    end
  end
end
