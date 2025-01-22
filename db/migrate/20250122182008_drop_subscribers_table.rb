class DropSubscribersTable < ActiveRecord::Migration[6.0]
  def change
    drop_table :subscribers do |t|
      t.string :email, null: false
      t.timestamps
      t.index :email, unique: true
    end
  end
end
