class CreateWashSaleLocks < ActiveRecord::Migration[8.1]
  def up
    # The lock belongs to the taxpayer, not to a bot: two bots holding the same name must not undo
    # each other's harvest, and deleting the bot that sold must not release the window.
    create_table :wash_sale_locks do |t|
      t.references :user, null: false, foreign_key: true, index: false
      t.references :asset, null: false, foreign_key: true
      t.datetime :buy_locked_until
      t.datetime :confirmed_locked_until
      # Which placement wrote the effective deadline. Two sales on the same day write the SAME
      # deadline, so the value cannot identify a claim: without this a failed placement's rollback
      # would match — and erase — a second, still-live placement's identical claim.
      t.string :claim_token
      t.timestamps
    end
    add_index :wash_sale_locks, %i[user_id asset_id], unique: true

    # Carry live per-bot locks up to their owner. MAX over the user's bots: two bots could each
    # hold a deadline for the same asset, and the longer window is the one that protects the loss.
    say_with_time 'moving buy locks to wash_sale_locks' do
      execute <<~SQL.squish
        INSERT INTO wash_sale_locks (user_id, asset_id, buy_locked_until, confirmed_locked_until, created_at, updated_at)
        SELECT bots.user_id, bot_index_assets.asset_id,
               MAX(bot_index_assets.buy_locked_until), MAX(bot_index_assets.confirmed_locked_until),
               CURRENT_TIMESTAMP, CURRENT_TIMESTAMP
        FROM bot_index_assets
        JOIN bots ON bots.id = bot_index_assets.bot_id
        WHERE bot_index_assets.buy_locked_until IS NOT NULL
           OR bot_index_assets.confirmed_locked_until IS NOT NULL
        GROUP BY bots.user_id, bot_index_assets.asset_id
      SQL
    end

    remove_column :bot_index_assets, :buy_locked_until
    remove_column :bot_index_assets, :confirmed_locked_until
  end

  # Reversible for real: a schema-only rollback would silently drop every live window and let the
  # fleet buy back into losses it is still protecting. The deadlines go back onto every composition
  # row of that taxpayer and asset, which is where the per-bot code reads them.
  def down
    # A lock with no composition row to go back to — its bot deleted, or an asset the bot never
    # recorded — has nowhere to live under the old schema. Refuse rather than drop it: a rollback
    # that silently releases a live window lets the fleet buy back into a loss it is protecting.
    orphaned = select_value(<<~SQL.squish)
      SELECT COUNT(*) FROM wash_sale_locks wsl
      WHERE wsl.buy_locked_until > CURRENT_TIMESTAMP
        AND NOT EXISTS (
          SELECT 1 FROM bot_index_assets bia
          JOIN bots ON bots.id = bia.bot_id
          WHERE bots.user_id = wsl.user_id AND bia.asset_id = wsl.asset_id)
    SQL
    if orphaned.to_i.positive?
      raise ActiveRecord::IrreversibleMigration,
            "#{orphaned} live wash-sale lock(s) have no composition row to move back to. " \
            'Rolling back would release them. Clear them deliberately, then retry.'
    end

    add_column :bot_index_assets, :buy_locked_until, :datetime
    add_column :bot_index_assets, :confirmed_locked_until, :datetime

    say_with_time 'moving buy locks back onto the composition rows' do
      execute <<~SQL.squish
        UPDATE bot_index_assets
        SET buy_locked_until = (
              SELECT wsl.buy_locked_until FROM wash_sale_locks wsl
              JOIN bots ON bots.id = bot_index_assets.bot_id
              WHERE wsl.user_id = bots.user_id AND wsl.asset_id = bot_index_assets.asset_id),
            confirmed_locked_until = (
              SELECT wsl.confirmed_locked_until FROM wash_sale_locks wsl
              JOIN bots ON bots.id = bot_index_assets.bot_id
              WHERE wsl.user_id = bots.user_id AND wsl.asset_id = bot_index_assets.asset_id)
        WHERE EXISTS (
          SELECT 1 FROM wash_sale_locks wsl
          JOIN bots ON bots.id = bot_index_assets.bot_id
          WHERE wsl.user_id = bots.user_id AND wsl.asset_id = bot_index_assets.asset_id)
      SQL
    end

    drop_table :wash_sale_locks
  end
end
