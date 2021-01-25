class AddPassphraseToApiKeys < ActiveRecord::Migration[5.2]
  def change
    add_column :api_keys, :encrypted_passphrase, :string
    add_column :api_keys, :encrypted_passphrase_iv, :string
  end
end
