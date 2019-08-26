class CreateBots < ActiveRecord::Migration[5.2]
  def change
    create_table :bots do |t|
      t.references :exchange, foreign_key: true
      t.integer :status
      t.integer :type
      t.references :user, foreign_key: true
      t.jsonb :settings, null: false, default: ""

      t.timestamps
    end
  end
end
