class AddAmountTypeToBotWebhooks < ActiveRecord::Migration[8.1]
  def change
    add_column :bot_webhooks, :amount_type, :integer, default: 0, null: false
  end
end
