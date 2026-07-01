class Transaction < ApplicationRecord
  belongs_to :bot
  belongs_to :exchange

  before_save :round_numeric_fields
  before_save :store_previous_quote_amount_exec
  before_save :store_previous_amount_exec
  after_create_commit -> { bot.broadcast_new_order(self) }
  after_create_commit lambda {
    api_key = ApiKey.find_by(user_id: bot.user_id, exchange_id: bot.exchange_id, key_type: :trading)
    AccountTransaction::SyncJob.perform_later(api_key) if api_key
  }
  after_update_commit -> { bot.broadcast_updated_order(self) }
  after_commit lambda {
                 Bot::UpdateMetricsJob.perform_later(bot) if metrics_relevant_change?
               }, on: %i[create update]
  # The quote spend cap is a BUY-side concept (a sell fill also changes quote_amount_exec, so guard
  # on side to keep sells from tripping the buy stop). The base sell cap is its sell-side mirror and
  # reacts to BASE execution, which can change while quote exec is nil/unchanged — hence its own
  # change detector.
  after_commit lambda {
                 bot.handle_quote_amount_limit_update if buy? && bot.class.include?(Bot::QuoteAmountLimitable) && custom_quote_amount_exec_changed?
               }, on: %i[create update]
  after_commit lambda {
                 bot.handle_base_amount_limit_update if sell? && bot.class.include?(Bot::BaseAmountLimitable) && base_cap_relevant_change?
               }, on: %i[create update]

  scope :for_bot, ->(bot) { where(bot_id: bot.id).order(created_at: :desc) }
  scope :today_for_bot, ->(bot) { for_bot(bot).where('created_at >= ?', Date.today.beginning_of_day) }
  scope :for_bot_by_status, ->(bot, status: :submitted) { where(bot_id: bot.id).where(status: status).order(created_at: :desc) }
  # Orders accepted by the exchange whose execution is not yet confirmed: submitted
  # rows with external_status open OR unknown. unknown rows are persisted at placement
  # (before the first confirmation fetch) and must be treated as in-flight everywhere.
  scope :waiting, -> { submitted.where(external_status: %i[open unknown]) }
  # Terminal rows the user can't act on anymore — explicit cancellation OR an
  # exchange-side abandonment (e.g. Kraken stopped returning the order). Grouped
  # under the "Cancelled" filter tab in the UI.
  scope :cancelled_or_abandoned, -> { where(external_status: %i[cancelled abandoned]) }

  validates :bot, presence: true

  enum :status, [
    :submitted, # Successfully sent and accepted by the exchange
    :failed,    # Attempted but failed (internal error or exchange rejection)
    :skipped    # Not even attempted (e.g., filtered, blocked, etc.)
  ]
  enum :side, %i[buy sell]
  enum :order_type, %i[market_order limit_order]
  enum :external_status, %i[unknown open closed cancelled abandoned]

  BTC = %w[XXBT XBT BTC].freeze

  # TODO: Migrate Transaction to directly reference assets instead of symbols
  def base_asset
    @base_asset ||= exchange.assets.find_by(symbol: base) ||
                    exchange.tickers.find_by(base: base)&.base_asset ||
                    exchange.tickers.find_by(quote: base)&.quote_asset ||
                    Asset.find_by(symbol: base)
  end

  def quote_asset
    @quote_asset ||= exchange.assets.find_by(symbol: quote) ||
                     exchange.tickers.find_by(quote: quote)&.quote_asset ||
                     exchange.tickers.find_by(base: quote)&.base_asset ||
                     Asset.find_by(symbol: quote)
  end

  # def count_by_status_and_exchange(status, exchange)
  #   joins(:bot).where(bots: { exchange_id: exchange.id }).where(status: status).count
  # end

  # def total_btc_sold
  #   total_btc_by_type('sell')
  # end

  # def total_btc_bought_day_ago
  #   joins(:bot)
  #     .where("bots.settings->>'type' = 'buy' AND bots.settings->>'base' IN (?) AND transactions.created_at < ? ",
  #            BTC, 1.days.ago)
  #     .sum(:amount).ceil(8)
  # end

  def update_with_order_data(order_data)
    # base/quote are historical snapshots — captured at first set and never rewritten by
    # subsequent order polls. Before the 2026-05-28 fix, `ticker.base_asset.symbol || base`
    # propagated any local asset.symbol mutation into transaction history (Bot 5 tx 168/169
    # went IBIT → LDRC this way during the stocks-rollout incident, with no Alpaca-side change).
    update({
      status: :submitted,
      external_status: order_data[:status],
      price: order_data[:price],
      amount: order_data[:amount],
      quote_amount: order_data[:quote_amount],
      base: base.presence || order_data[:ticker]&.base_asset&.symbol,
      quote: quote.presence || order_data[:ticker]&.quote_asset&.symbol,
      side: order_data[:side],
      order_type: order_data[:order_type],
      amount_exec: order_data[:amount_exec],
      quote_amount_exec: order_data[:quote_amount_exec]
    }.compact)
  end

  def imported?
    external_id&.start_with?('imported_')
  end

  def cancel
    return Result::Failure.new(I18n.t('bot.messages.cannot_cancel_imported_order')) if imported?

    result = bot.cancel_order(order_id: external_id)
    return result if result.failure?

    Bot::FetchAndUpdateOrderJob.perform_later(self, update_missed_quote_amount: true)
    Result::Success.new(self)
  end

  private

  # def total_btc_by_type(type)
  #   joins(:bot)
  #     .where("bots.settings->>'type' = ? AND bots.settings->>'base' IN (?)", type, BTC)
  #     .sum(:amount).ceil(8)
  # end

  def round_numeric_fields
    self.price = price&.round(18)
    self.amount = amount&.round(18)
    self.amount_exec = amount_exec&.round(18)
    self.bot_quote_amount = bot_quote_amount&.round(18)
    self.quote_amount = quote_amount&.round(18)
    self.quote_amount_exec = quote_amount_exec&.round(18)
  end

  def store_previous_quote_amount_exec
    @previous_quote_amount_exec = quote_amount_exec_was
  end

  def custom_quote_amount_exec_changed?
    quote_amount_exec != @previous_quote_amount_exec && quote_amount_exec.present? && quote_amount_exec.positive?
  end

  # Metrics recompute when an order's CONTRIBUTION changes: its quote execution, or its transition to
  # `closed`. The metrics count an order only once it is closed (via the legacy fallback even if exec
  # amounts are still nil), so a close with no quote-exec change must still invalidate the cache —
  # otherwise that fill would leave the 30-day metrics cache stale.
  def metrics_relevant_change?
    custom_quote_amount_exec_changed? || (saved_change_to_external_status? && closed?)
  end

  def store_previous_amount_exec
    @previous_amount_exec = amount_exec_was
  end

  # Base-execution change detector — the sell cap's mirror of custom_quote_amount_exec_changed?.
  def custom_amount_exec_changed?
    amount_exec != @previous_amount_exec && amount_exec.present? && amount_exec.positive?
  end

  # The base cap counts a sell once it is closed (via the requested-amount fallback even if amount_exec
  # is still nil), so a close with no amount_exec change must still re-evaluate the cap — otherwise a
  # nil-exec close that reaches the cap would skip the stop/notification. Mirrors metrics_relevant_change?.
  def base_cap_relevant_change?
    custom_amount_exec_changed? || (saved_change_to_external_status? && closed?)
  end
end
