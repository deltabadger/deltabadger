class EnforceNftIdsUnique < ActiveRecord::Migration[6.0]
  def change
    add_index :subscriptions, :nft_id, unique: true, where: "nft_id IS NOT NULL"
  end
end
