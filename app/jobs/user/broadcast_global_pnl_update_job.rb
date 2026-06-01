class User::BroadcastGlobalPnlUpdateJob < ApplicationJob
  queue_as :default

  # Computes the global PnL live (warming the per-bot + FX caches as a side effect) and
  # broadcasts the refreshed `global-pnl` target. Triggered by the /bots index on-connect
  # when the cache-only snapshot was still loading — independent of the per-bot PnL jobs,
  # so it also covers the "all bot caches warm but an FX rate is cold" case.
  def perform(user)
    user.broadcast_global_pnl_update
  end
end
