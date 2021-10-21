class CreateBotsTotalAmounts < ActiveRecord::Migration[5.2]
  def change
    create_view :bots_total_amounts, materialized: true
  end
end
