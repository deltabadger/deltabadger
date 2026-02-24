class RenameExchangesBitMartToBitmart < ActiveRecord::Migration[8.1]
  def up
    execute "UPDATE exchanges SET type = 'Exchanges::Bitmart' WHERE type = 'Exchanges::BitMart'"
  end

  def down
    execute "UPDATE exchanges SET type = 'Exchanges::BitMart' WHERE type = 'Exchanges::Bitmart'"
  end
end
