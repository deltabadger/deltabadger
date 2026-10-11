class AddDispatchIntentsAndOrderRunIdentity < ActiveRecord::Migration[8.1]
  def change
    add_column :bots, :placement_intents, :json, null: false, default: {}
    add_column :transactions, :run_identity, :json
  end
end
