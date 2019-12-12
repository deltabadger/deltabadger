class AddPayments < ActiveRecord::Migration[5.2]
  def change
    create_table :payments do |t|
      t.string :payment_id
      t.integer :status
      t.decimal :total
      t.integer :currency
      t.references :user, index: true

      t.timestamps
    end
  end
end
