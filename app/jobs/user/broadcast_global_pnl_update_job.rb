class User::BroadcastGlobalPnlUpdateJob < ApplicationJob
  queue_as :default
  # One live pass per user at a time. The dashboard retries this when the figure does not
  # arrive, and at eighty bots a pass can outlast the retry delay — without this, the retry
  # would start a second full-account pass instead of waiting for the first. A job that has
  # already failed is no longer in flight, so a genuine retry still gets through.
  # duration must outlast the worst pass: Solid Queue's maintenance can delete a semaphore once
  # its duration elapses, even while the job that holds it is still running, which would let a
  # reload start a second full-account pass.
  limits_concurrency to: 1, key: ->(user) { "global_pnl_user_#{user.id}" },
                     on_conflict: :discard, duration: 10.minutes

  # Computes the global PnL live (warming the per-bot + FX caches as a side effect) and
  # broadcasts the refreshed `global-pnl` target. Triggered by the /bots index on-connect
  # when the cache-only snapshot was still loading — independent of the per-bot PnL jobs,
  # so it also covers the "all bot caches warm but an FX rate is cold" case.
  def perform(user)
    user.broadcast_global_pnl_update
  end
end
