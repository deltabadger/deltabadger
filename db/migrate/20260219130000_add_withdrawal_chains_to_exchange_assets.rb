class AddWithdrawalChainsToExchangeAssets < ActiveRecord::Migration[8.1]
  def change
    add_column :exchange_assets, :withdrawal_chains, :json
  end
end
