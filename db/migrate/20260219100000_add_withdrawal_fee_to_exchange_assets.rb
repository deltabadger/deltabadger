class AddWithdrawalFeeToExchangeAssets < ActiveRecord::Migration[8.1]
  def change
    add_column :exchange_assets, :withdrawal_fee, :string
    add_column :exchange_assets, :withdrawal_fee_updated_at, :datetime
  end
end
