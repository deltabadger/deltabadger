class RemoveExternalIdFromExchanges < ActiveRecord::Migration[6.0]
  def change
    remove_column :exchanges, :external_id, :string
  end
end
