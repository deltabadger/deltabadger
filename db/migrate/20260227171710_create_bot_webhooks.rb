class CreateBotWebhooks < ActiveRecord::Migration[8.1]
  def change
    create_table :bot_webhooks do |t|
      t.references :bot, null: false, foreign_key: true
      t.string :token, null: false
      t.integer :direction, null: false, default: 0
      t.decimal :amount, null: false

      t.timestamps
    end

    add_index :bot_webhooks, :token, unique: true
  end
end
