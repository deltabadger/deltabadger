# Records the asset of every ledger row of a user that has none, whatever its age — the one-time
# backfill for rows stored before the ledger recorded assets. Safe to run again: a recorded asset is
# never replaced, and a row that still resolves to nothing stays NULL and is read by its symbol.
class AccountTransaction::ResolveAssetsJob < ApplicationJob
  queue_as :low_priority

  def perform(user_id)
    AccountTransaction.resolve_base_assets!(AccountTransaction.where(user_id: user_id))
  end
end
