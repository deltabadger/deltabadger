class RenameSequenceNumberInSubscriptions < ActiveRecord::Migration[6.0]
  def change
    rename_column :subscriptions, :sequence_number, :nft_id
  end
end
