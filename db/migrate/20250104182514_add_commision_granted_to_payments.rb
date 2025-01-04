class AddCommisionGrantedToPayments < ActiveRecord::Migration[6.0]
  def up
    add_column :payments, :commission_granted, :boolean, default: false

    Payment.find_each do |payment|
      # When this migration was created, all commissions were granted
      if payment.commission.positive?
        payment.update!(commission_granted: true)
      end
    end
  end

  def down
    remove_column :payments, :commission_granted
  end
end
