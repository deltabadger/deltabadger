class MigrateDualAssetBotsToMultiAsset < ActiveRecord::Migration[8.1]
  # Rails would otherwise wrap this in a transaction on the PRIMARY connection — but Solid Queue is
  # a separate database whose writes commit independently. Under one transaction a failure late in
  # the run would roll every type back to the pair class while the repointed job GlobalIDs stayed
  # committed: bots addressed by jobs naming the class they are not. Each bot's conversion commits
  # on its own instead, and the converter is idempotent, so a re-run finishes what a crash left.
  disable_ddl_transaction!

  def up
    converted, skipped = Bot::DualToComposition.run!

    say "Converted #{converted.size} pair bot(s) into two-asset baskets."
    skipped.each { |id, reason| say "Skipped bot #{id}: #{reason}", true }
    say 'Run bin/rails bots:migrate_dual_to_multi once those bots are idle.' if skipped.any?
  end

  # Irreversible on purpose: the pair shape cannot represent a basket, so rolling back after any
  # later edit would silently discard members or weights.
  def down
    raise ActiveRecord::IrreversibleMigration
  end
end
