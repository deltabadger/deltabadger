class CreateIdempotencyKeys < ActiveRecord::Migration[8.1]
  def change
    create_table :idempotency_keys do |t|
      t.references :user, null: false, foreign_key: true
      t.string :key, null: false
      t.string :request_fingerprint, null: false
      t.string :state, null: false, default: 'in_progress'
      t.integer :response_status
      t.text :response_body
      t.datetime :locked_at, null: false
      t.timestamps
    end

    add_index :idempotency_keys, [:user_id, :key], unique: true
    add_index :idempotency_keys, :created_at
  end
end
