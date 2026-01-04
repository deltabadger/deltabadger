class AddEncryptedValueToAppConfigs < ActiveRecord::Migration[6.0]
  def change
    add_column :app_configs, :encrypted_value, :text
    add_column :app_configs, :encrypted_value_iv, :string
    remove_column :app_configs, :value, :text
  end
end
