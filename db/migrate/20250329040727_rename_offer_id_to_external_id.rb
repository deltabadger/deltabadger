class RenameOfferIdToExternalId < ActiveRecord::Migration[6.0]
  def change
    rename_column :transactions, :offer_id, :external_id
    rename_column :daily_transaction_aggregates, :offer_id, :external_id

    # add_index :transactions, :external_id, unique: true
  end
end
