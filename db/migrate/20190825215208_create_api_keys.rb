class CreateApiKeys < ActiveRecord::Migration[5.2]
  def change
    create_table :api_keys do |t|
      t.references :exchange, foreign_key: true, null: false
      t.references :user, foreign_key: true, null: false
      t.string :encrypted_key
      t.string :encrypted_key_iv

      t.timestamps
    end
  end
end
