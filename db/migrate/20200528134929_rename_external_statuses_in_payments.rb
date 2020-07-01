class RenameExternalStatusesInPayments < ActiveRecord::Migration[5.2]
  def change
    rename_column :payments, :globee_statuses, :external_statuses
  end
end
