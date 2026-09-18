class MigrateSingleAssetBotsToMultiAsset < ActiveRecord::Migration[8.1]
  # Solid Queue is a separate database whose writes commit independently: under one transaction a late
  # failure would roll every type back while the repointed job GlobalIDs stayed. Each bot commits on its
  # own instead, and the converter is idempotent, so a re-run finishes what a crash left.
  disable_ddl_transaction!

  def up
    converted, skipped = Bot::SingleToComposition.run!

    say "Converted #{converted.size} single-asset bot(s) into one-asset multi-asset bots."
    skipped.each { |id, reason| say "Skipped #{id}: #{reason}", true }
    say "#{skipped.size} left for the recurring conversion." if skipped.any?
  end

  # Irreversible on purpose: a multi-asset bot edited after the conversion cannot be put back.
  def down
    raise ActiveRecord::IrreversibleMigration
  end
end
