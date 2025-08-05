class CreateCard < ActiveRecord::Migration[6.0]
  def change
    create_table :cards do |t|
      t.references :user, null: false, foreign_key: true
      t.string :token, null: false
      t.string :first_transaction_id, null: false
    end
  end
end
