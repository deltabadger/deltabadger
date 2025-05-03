class JsonToJsonb < ActiveRecord::Migration[6.0]
  def up
    change_column :portfolios, :compare_to, :jsonb
    change_column :bots, :transient_data, :jsonb
  end

  def down
    change_column :portfolios, :compare_to, :json
    change_column :bots, :transient_data, :json
  end
end
