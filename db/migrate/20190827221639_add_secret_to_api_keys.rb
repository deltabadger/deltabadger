class AddSecretToApiKeys < ActiveRecord::Migration[5.2]
  def change
    add_column :api_keys, :encrypted_secret, :string
    add_column :api_keys, :encrypted_secret_iv, :string
  end
end
