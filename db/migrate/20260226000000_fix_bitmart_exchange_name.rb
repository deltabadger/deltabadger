class FixBitmartExchangeName < ActiveRecord::Migration[8.1]
  def up
    execute "UPDATE exchanges SET name = 'Bitmart' WHERE type = 'Exchanges::Bitmart' AND name != 'Bitmart'"
  end

  def down
    # no-op — the name was inconsistent before
  end
end
