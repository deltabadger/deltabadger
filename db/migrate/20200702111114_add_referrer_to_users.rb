class AddReferrerToUsers < ActiveRecord::Migration[5.2]
  def change
    add_column :users, :referrer_id, :bigint
    add_foreign_key :users, :affiliates, column: :referrer_id
  end
end
