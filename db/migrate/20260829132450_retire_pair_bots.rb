class RetirePairBots < ActiveRecord::Migration[8.1]
  # Solid Queue is a separate database: a rollback here could not take the repointed GlobalIDs with
  # it, so each bot commits on its own and the converter is idempotent.
  disable_ddl_transaction!

  # The class this migration retires is already gone from the image, so every pair row left — busy
  # at the earlier migration, or on an install that skipped it — is converted here whatever its
  # state. Nothing raises: a row that could not be converted cleanly is still made loadable and
  # stopped, because a failed migration is a container that restarts into the same failure.
  def up
    converted, degraded, failed = Bot::DualToComposition.finalize!

    say "Converted #{converted.size + degraded.size} remaining pair bot(s)."
    degraded.each { |id, gaps| say "Bot #{id}: converted with #{gaps.join(', ')}; not left running.", true }
    failed.each do |id, error|
      if id == 'queue'
        say "Queued jobs could not be repointed (#{error}); a job still naming the old class fails when a worker picks it up.", true
      else
        say "Bot #{id}: converted as far as it could be (#{error}); check it.", true
      end
    end
  end

  # Irreversible on purpose: the pair shape cannot represent a basket, and the class that read it no
  # longer exists.
  def down
    raise ActiveRecord::IrreversibleMigration
  end
end
