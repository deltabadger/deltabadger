class JsonToJsonb < ActiveRecord::Migration[6.0]
  def change
    change_column :portfolios, :compare_to, :jsonb
    change_column :bots, :transient_data, :jsonb
  end
end
