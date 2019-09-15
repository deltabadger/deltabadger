class CreateTransactions < ActiveRecord::Migration[5.2]
  def change
    create_table :transactions do |t|
      t.references :bot, foreign_key: true
      t.uuid :offer_id
      t.decimal :rate
      t.decimal :amount
      t.string :market
      t.integer :status
      t.integer :currency

      t.timestamps
    end
  end
end
