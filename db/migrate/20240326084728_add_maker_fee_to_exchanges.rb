class AddMakerFeeToExchanges < ActiveRecord::Migration[6.0]
  def change
    add_column :exchanges, :maker_fee, :string
  end
end
