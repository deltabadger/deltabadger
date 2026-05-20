class CreateBotActivityLogs < ActiveRecord::Migration[8.1]
  def change
    create_table :bot_activity_logs do |t|
      t.references :bot, null: false, foreign_key: true
      t.integer :level, default: 0, null: false
      t.string :event, null: false
      t.string :message
      t.json :details, default: {}, null: false
      t.datetime :created_at, null: false
    end
    add_index :bot_activity_logs, %i[bot_id created_at]
  end
end
