class RemoveWithdrawalFeeFromExchanges < ActiveRecord::Migration[8.1]
  def change
    remove_column :exchanges, :withdrawal_fee, :string

    # Clear mock withdrawal fees left by dry mode
    ExchangeAsset.where(withdrawal_fee: '0.001').update_all(withdrawal_fee: nil, withdrawal_fee_updated_at: nil)
  end
end
