# Kraken's "EGeneral:Permission denied" used to sit in Exchanges::Kraken::ERRORS[:invalid_key], so
# a key that merely lacked Data → Query Ledger Entries — needed by the tracker's Ledgers call, and
# by nothing a bot does — was flipped to :incorrect on the first tracker sync. That status removes
# the key from every `status: :correct`-scoped sync, and Kraken shows a private key exactly once, so
# the affected users cannot recover by re-entering credentials they no longer have. Give them their
# key back (issue #153).
#
# Two cohorts, because `last_sync_error` only exists since 20260815160000 while the misclassification
# is far older:
#
#   1. Keys whose recorded error names the permission denial. Exact.
#   2. Keys condemned with NO recorded error at all. Unattributable — this cohort holds both the
#      pre-column victims of this bug and genuinely dead keys, and nothing stored can separate them.
#
# Restoring cohort 2 is safe because the app re-verifies it for free: a restored key rejoins the
# `:correct`-scoped syncs, and a credential that really is dead answers `EAPI:Invalid key`, which the
# (untouched) invalid-key path condemns again — on the next scheduled sync, with no user action. The
# cost of being wrong is one failed sync; the cost of skipping the cohort is a user stranded with a
# working key they cannot use.
#
# Scoped to Kraken trading keys: this misclassification never reached another venue (no other
# ERRORS[:invalid_key] list held a scope-only error), and the two sync jobs that condemn keys both
# read `key_type: :trading`, so a withdrawal key was never touched by it either.
#
# Self-contained on purpose: raw SQL and literal enum integers, no ApiKey model. A historical
# migration that depends on head-of-branch code breaks fresh installs migrating from zero.
# status: 0 pending_validation, 1 correct, 2 incorrect, 3 pending_activation. key_type: 0 trading.
class RestoreKrakenKeysCondemnedByPermissionError < ActiveRecord::Migration[8.1]
  def up
    execute <<~SQL.squish
      UPDATE api_keys SET status = 1
      WHERE status = 2
        AND key_type = 0
        AND (last_sync_error LIKE '%EGeneral:Permission denied%' OR last_sync_error IS NULL)
        AND exchange_id IN (SELECT id FROM exchanges WHERE type = 'Exchanges::Kraken')
    SQL
  end

  def down
    # Irreversible by design: after this runs the app itself re-condemns any key that is actually
    # dead, so there is no "before" state worth restoring.
  end
end
