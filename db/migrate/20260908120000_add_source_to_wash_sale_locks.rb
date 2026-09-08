class AddSourceToWashSaleLocks < ActiveRecord::Migration[8.1]
  def change
    # Where the lock came from: 'bot' when a bot's own sale armed it at placement, 'ledger' when the
    # account walk found a disposal — including one the user made on the exchange's own website. The
    # protection table says so on the row, because a countdown with nothing behind it reads as a bug.
    add_column :wash_sale_locks, :source, :string, default: 'bot', null: false
  end
end
