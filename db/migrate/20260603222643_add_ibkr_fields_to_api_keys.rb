class AddIbkrFieldsToApiKeys < ActiveRecord::Migration[8.1]
  def change
    add_column :api_keys, :access_token, :text
    add_column :api_keys, :rsa_signature_key, :text
    add_column :api_keys, :rsa_encryption_key, :text
    add_column :api_keys, :dh_param, :text
    add_column :api_keys, :ibkr_realm, :string
  end
end
