# Re-derives the index composition after the user has changed what the index IS — the coins slider,
# most of all. Out of the request because deriving it reads live prices (Ticker#priced?), which this
# codebase keeps out of the request path, and because a price read raises on a network blip: inline,
# that would 500 a settings change that had already been saved.
class Bot::ResyncIndexCompositionJob < ApplicationJob
  queue_as :default

  # Deliberately NOT a BotJob: that base class serialises on `exchange_<name_id>`, one job at a time
  # per venue, so the tables would queue behind whatever trading the venue is doing — the wait this
  # is meant to remove. It needs no trading lock either: it writes only bot_index_assets, and the
  # rebalancer and the liquidation each re-derive the composition themselves before they act. Its
  # exchange traffic is one whole-market price read behind the per-exchange one-minute cache.
  #
  # Serialised per bot. Two refreshes in flight can each be working from a different reading of
  # num_coins, and the one that finishes last wins — leaving a composition the settings no longer
  # ask for and nothing to correct it before the next buy. Queued runs re-read the bot, so the last
  # one always settles on the current answer.
  limits_concurrency to: 1, key: ->(bot) { "index_composition_#{bot.id}" }, duration: 1.minute

  # Reads plus an idempotent upsert — nothing here places an order, so replaying it is free.
  retry_on Client::TransientNetworkError, wait: :polynomially_longer, attempts: 3

  def perform(bot)
    bot.refresh_index_composition
    # The tables only, not Bot::BroadcastMetricsUpdateJob: that one also re-renders the chart, whose
    # candle series a composition change cannot alter — and waiting on it is what made the split
    # arrive with the chart redraw rather than on its own. Broadcast either way, since a failed
    # refresh leaves the old composition standing and the tables should show what is on record.
    bot.broadcast_metrics_panel
  end
end
