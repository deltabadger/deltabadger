class AddGermanTradingAgreementToApiKeys < ActiveRecord::Migration[5.2]
  def change
    add_column :api_keys, :german_trading_agreement, :boolean
  end
end
