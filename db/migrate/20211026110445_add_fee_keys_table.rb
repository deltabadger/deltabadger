class AddFeeKeysTable < ActiveRecord::Migration[5.2]
  def change
    create_table :fee_api_keys, force: :cascade do |t|
      t.references :exchange, foreign_key: true, null: false
      t.string :encrypted_key
      t.string :encrypted_key_iv
      t.string :encrypted_secret
      t.string :encrypted_secret_iv
      t.string :encrypted_passphrase
      t.string :encrypted_passphrase_iv
    end
  end
end
