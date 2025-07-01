class AddTypeToPayment < ActiveRecord::Migration[6.0]
  def up
    add_column :payments, :type, :string
    add_index :payments, :type

    Payment.where(payment_type: 0).update_all(type: 'Payments::Btcpay')
    Payment.where(payment_type: 1).update_all(type: 'Payments::Wire')
    Payment.where(payment_type: 2).update_all(type: 'Payments::Stripe')
    Payment.where(payment_type: 3).update_all(type: 'Payments::Zen')

    remove_column :payments, :payment_type
  end

  def down
    add_column :payments, :payment_type, :integer
    add_index :payments, :payment_type

    Payment.where(type: 'Payments::Btcpay').update_all(payment_type: 0)
    Payment.where(type: 'Payments::Wire').update_all(payment_type: 1)
    Payment.where(type: 'Payments::Stripe').update_all(payment_type: 2)
    Payment.where(type: 'Payments::Zen').update_all(payment_type: 3)

    remove_column :payments, :type
    remove_index :payments, :type
  end
end
