class MakeWashSaleEnabledTriState < ActiveRecord::Migration[8.1]
  def up
    # nil is "never decided". The prompt keeps appearing until the user answers — including to say
    # no — because leaving a toggle off is indistinguishable from never having looked at it.
    change_column_null :users, :wash_sale_enabled, true
    change_column_default :users, :wash_sale_enabled, from: false, to: nil

    # Anyone who had already answered under the two-state column has decided by definition; anyone
    # who merely carried the default has not.
    execute <<~SQL.squish
      UPDATE users SET wash_sale_enabled = NULL
      WHERE wash_sale_enabled = 0 AND wash_sale_prompted_at IS NULL
    SQL

    remove_column :users, :wash_sale_prompted_at
  end

  def down
    add_column :users, :wash_sale_prompted_at, :datetime
    execute 'UPDATE users SET wash_sale_prompted_at = CURRENT_TIMESTAMP WHERE wash_sale_enabled IS NOT NULL'
    execute 'UPDATE users SET wash_sale_enabled = 0 WHERE wash_sale_enabled IS NULL'
    change_column_default :users, :wash_sale_enabled, from: nil, to: false
    change_column_null :users, :wash_sale_enabled, false
  end
end
