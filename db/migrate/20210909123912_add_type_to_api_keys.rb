class AddTypeToApiKeys < ActiveRecord::Migration[5.2]
  def change
    add_column :api_keys, :type, :integer, null: false, default: 0
  end
end
