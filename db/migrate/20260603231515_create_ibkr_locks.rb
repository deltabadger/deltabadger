class CreateIbkrLocks < ActiveRecord::Migration[8.1]
  def change
    create_table :ibkr_locks do |t|
      t.string :key, null: false
      t.string :owner, null: false
      t.datetime :expires_at, null: false
      t.timestamps
    end
    # The UNIQUE index on :key is the atomic, cross-process arbiter for the lock
    # (acquire = INSERT; a second INSERT with the same key fails with RecordNotUnique).
    add_index :ibkr_locks, :key, unique: true
    add_index :ibkr_locks, :expires_at
  end
end
