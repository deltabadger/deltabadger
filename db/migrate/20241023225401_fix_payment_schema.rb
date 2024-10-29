class FixPaymentSchema < ActiveRecord::Migration[6.0]
  def up
    change_column :payments, :total, :decimal, precision: 10, scale: 2
    change_column :payments, :commission, :decimal, precision: 10, scale: 2

    change_column_default :payments, :payment_type, from: 0, to: nil
    change_column_default :payments, :discounted, from: false, to: nil
    change_column_default :payments, :commission, from: 0, to: nil

    change_column_null :payments, :status, false
    change_column_null :payments, :user_id, false
    change_column_null :payments, :total, false
    change_column_null :payments, :currency, false

    add_index :payments, :currency
    add_index :payments, :status
    add_index :payments, :payment_type
  end

  def down
    change_column :payments, :total, :decimal, precision: nil, scale: nil
    change_column :payments, :commission, :decimal, precision: nil, scale: nil

    change_column_default :payments, :payment_type, from: nil, to: 0
    change_column_default :payments, :discounted, from: nil, to: false
    change_column_default :payments, :commission, from: nil, to: 0

    change_column_null :payments, :status, true
    change_column_null :payments, :user_id, true
    change_column_null :payments, :total, true
    change_column_null :payments, :currency, true

    remove_index :payments, :currency
    remove_index :payments, :status
    remove_index :payments, :payment_type
  end
end
