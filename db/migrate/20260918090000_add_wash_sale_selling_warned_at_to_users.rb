# When the account was told what wash-sale protection cannot cover while a multi-asset bot sells.
# Asked once per account: nil until the user acknowledges it.
class AddWashSaleSellingWarnedAtToUsers < ActiveRecord::Migration[8.1]
  def change
    add_column :users, :wash_sale_selling_warned_at, :datetime
  end
end
