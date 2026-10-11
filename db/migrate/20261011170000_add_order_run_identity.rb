class AddOrderRunIdentity < ActiveRecord::Migration[8.1]
  def change
    add_column :transactions, :run_identity, :json
  end
end
