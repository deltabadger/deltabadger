class ChangePaymentsExternalStatusesToJson < ActiveRecord::Migration[6.0]
  def up
    rename_column :payments, :external_statuses, :external_statuses_bak
    add_column :payments, :external_statuses, :jsonb, default: []
    Payment.all.each do |payment|
      payment.update!(external_statuses: payment.external_statuses_bak.split(', '))
    end
    remove_column :payments, :external_statuses_bak
  end

  def down
    rename_column :payments, :external_statuses, :external_statuses_bak
    add_column :payments, :external_statuses, :string, default: ''
    Payment.all.each do |payment|
      payment.update!(external_statuses: payment.external_statuses_bak.join(', '))
    end
    remove_column :payments, :external_statuses_bak
  end
end
