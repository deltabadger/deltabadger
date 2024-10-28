class FixPaymentSchema < ActiveRecord::Migration[6.0]
  def up
    change_column :payments, :total, :decimal, precision: 10, scale: 2
    change_column :payments, :commission, :decimal, precision: 10, scale: 2

    change_column_default :payments, :payment_type, from: 0, to: nil
    change_column_default :payments, :discounted, from: false, to: nil
    change_column_default :payments, :commission, from: 0, to: nil

    # change_column_null :payments, :status, false
    # change_column_null :payments, :user_id, false
    # change_column_null :payments, :total, false

    change_column_null :payments, :status, false
    change_column_null :payments, :user_id, false
    change_column_null :payments, :external_statuses, true
    change_column_null :payments, :btc_total, true
    change_column_null :payments, :btc_paid, true
    change_column_null :payments, :commission, true
    change_column_null :payments, :btc_commission, true
    change_column_null :payments, :discounted, true
    change_column_null :payments, :subscription_plan_variant_id, true
    change_column_null :payments, :country, true
    change_column_null :payments, :payment_type, true

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

    # change_column_null :payments, :status, true
    # change_column_null :payments, :user_id, true
    # change_column_null :payments, :total, true

    change_column_null :payments, :status, true
    change_column_null :payments, :user_id, true
    change_column_null :payments, :external_statuses, false
    change_column_null :payments, :btc_total, false
    change_column_null :payments, :btc_paid, false
    change_column_null :payments, :commission, false
    change_column_null :payments, :btc_commission, false
    change_column_null :payments, :discounted, false
    change_column_null :payments, :subscription_plan_variant_id, false
    change_column_null :payments, :country, false
    change_column_null :payments, :payment_type, false

    remove_index :payments, :currency
    remove_index :payments, :status
    remove_index :payments, :payment_type
  end
end
