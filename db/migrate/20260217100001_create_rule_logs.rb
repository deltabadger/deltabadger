class CreateRuleLogs < ActiveRecord::Migration[8.1]
  def change
    create_table :rule_logs do |t|
      t.references :rule, null: false, foreign_key: true
      t.integer :status, default: 0, null: false
      t.string :message
      t.json :details, default: {}, null: false
      t.datetime :created_at, null: false
    end
    add_index :rule_logs, [:rule_id, :created_at]
  end
end
