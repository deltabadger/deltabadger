class Bot < ApplicationRecord
  include Automation::Statusable
  include Automation::Configurable
  include Automation::Executable
  include Automation::ExchangeConnectable
  include Automation::Dryable # decorators for: api_key
  include Automation::Labelable
  include Automation::DomIdable

  include Typeable
  include Rankable
  include Notifyable
  include ExchangeUser
  include ActivityLoggable
  include ChartSeries # chart marked at market (candle grid), shared by every measurable

  belongs_to :user
  has_many :transactions, dependent: :destroy

  # The dashboard order, dragged by the user. `:id` is not decoration: two bots created in
  # overlapping transactions read the same `maximum(:position)` and land on the same number, and
  # the list still has to come back the same way on every page load. The next drag renumbers them
  # apart — Bots::ReordersController rewrites the whole sequence, not just the rows it was sent.
  scope :ordered, -> { order(:position, :id) }

  before_create :assign_position

  # Every start path — the button, a stale tab, BotApi::Bots::Start — goes through valid?(:start),
  # so one validation is the whole gate: an archived bot is reactivated first or not at all.
  validate :not_archived, on: :start

  before_save :store_previous_exchange_id
  after_update_commit :broadcast_status_bar_update, if: -> { saved_change_to_status? && !@skip_status_bar_broadcast }
  after_update_commit :broadcast_status_button_update, if: :saved_change_to_status?
  after_update_commit :broadcast_columns_lock_update, if: :saved_change_to_status?
  after_update_commit -> { Bot::UpdateMetricsJob.perform_later(self) if custom_exchange_id_changed? }

  # Fix C: drives the "temporarily unavailable" hint shown next to a disabled start button so the
  # frozen toggle isn't silent. True when the bot's ticker(s) aren't currently available/
  # trading-enabled on the exchange — the usual reason validate_tickers_available blocks :start.
  # Subtypes list their relevant tickers via #tickers_for_start; types that don't override show no hint.
  def start_blocked_by_unavailable_ticker?
    return false unless exchange_id?

    relevant = tickers_for_start
    return false if relevant.blank?

    relevant.any? { |t| t.nil? || !t.available? || !t.trading_enabled? }
  end

  # Overridden by bot subtypes that trade specific tickers. Default: no ticker info → no hint.
  def tickers_for_start
    []
  end

  # Mark a metrics payload that had to fall back to the last executed transaction price because the
  # live read failed. Every bot type does that fallback — it is better than a blank page — but the
  # view then renders a carried-over number as "Portfolio value" with nothing to distinguish it from
  # a live one, and with a rejected key it simply stops moving for weeks. The flag drives the
  # already-translated `bot.details.stats.exchange_unavailable_html` notice.
  def stale(metrics_data)
    metrics_data[:prices_stale] = true
    metrics_data
  end

  # Hidden from the list, keeps its history, trades nothing. Archiving goes through #stop so the
  # job cancellation and the settings-dirty guard stay in one place — an archived bot must never
  # leave a tick scheduled behind it.
  def archive
    stop && update(status: :archived)
  end

  # status_was, not archived?: both #start implementations assign :scheduled before they validate,
  # so the only place the archive is still visible at validation time is the persisted value.
  def not_archived
    errors.add(:status, :archived, message: I18n.t('errors.bots.archived')) if status_was == 'archived'
  end

  # Always :stopped, never :created — a never-run bot renders identically either way (no
  # last_action_job_at, so a plain fresh Start), while :created is the one status that would put
  # the bot back under validate_bot_exchange and refuse to reactivate a delisted pair. Reading
  # started_at to tell the two apart would not work anyway: the limit concerns decorate it.
  #
  # Idempotent on purpose: a stale second Reactivate (double click, retried request) must not
  # write :stopped over a bot that has since been started again — that would halt it without
  # cancelling its scheduled tick.
  def unarchive
    return true unless archived?

    update(status: :stopped)
  end

  def last_transaction
    transactions.where(transaction_type: 'REGULAR').order(created_at: :desc).limit(1).last
  end

  def last_successful_transaction
    transactions.where(status: %i[submitted skipped]).order(created_at: :desc).limit(1).last
  end

  def successful_transaction_count
    transactions.where(status: %i[submitted skipped]).order(created_at: :desc).count
  end

  # Direction defaults — bots are buy-only unless they include Bot::Reversible (DcaSingleAsset),
  # which overrides these. Lets the shared trigger concerns read direction uniformly across bot
  # types without each having to guard for the predicate's existence.
  def buying?
    true
  end

  def selling?
    false
  end

  # How a sell tick is sized. Both false for buy-only types, so Bot::SmartIntervalable and the
  # settings partials it shares with DcaDualAsset / DcaIndex can read them unguarded, exactly like
  # selling? above. Bot::Reversible overrides them.
  def sells_base_amount?
    false
  end

  def sells_quote_amount?
    false
  end

  # Only Bot::Reversible (DcaSingleAsset) can flip direction. Trigger concerns are shared with
  # buy-only bot types, so the flip action and the ⇄ control must be gated on this.
  def reversible?
    false
  end

  # Decode the merged trigger "mode" select (issues #1/#2) back into the stored
  # (timing_condition, action) pair, for whichever side(s) the form submitted. The UI collapses the
  # old separate action + timing dropdowns into one direction-aware select; parse_params in each
  # limitable merges this expansion. `has_timing: false` for price-drop (no timing field — its pause
  # latches, so only the action is written). The `…_mode` key itself is never stored.
  #   restrict -> while + pause   start -> after + pause   flip -> after + <opposite>-side action
  def expand_trigger_mode(params, base, has_timing:)
    %W[#{base} sell_#{base}].each_with_object({}) do |prefix, out|
      mode = params[:"#{prefix}_mode"].presence
      next unless mode

      flip_action = prefix.start_with?('sell_') ? 'start_buying' : 'start_selling'
      out["#{prefix}_action"] = mode == 'flip' ? flip_action : 'pause'
      next unless has_timing

      out["#{prefix}_timing_condition"] = mode == 'restrict' ? 'while' : 'after'
    end
  end

  # Net of the bot's CLOSED executed buys minus closed executed sells, clamped ≥ 0 (closed legacy rows
  # with no amount_exec fall back to the requested `amount`). A net-holdings accessor for display — it
  # is NO LONGER a sell cap: a selling bot may liquidate the whole wallet, not just what it accumulated
  # (sellable_base_amount). Note this differs slightly from the metrics' net_base, which also counts
  # real partial executions on non-closed rows.
  def total_amount
    filled = Arel.sql('COALESCE(amount_exec, amount)')
    net = transactions.submitted.buy.closed.sum(filled) - transactions.submitted.sell.closed.sum(filled)
    [net, 0].max
  end

  # A tile's profit in USD — every tile reads in the same currency as the account total, so
  # the two formats stay comparable across bots quoted in EUR, USDT or anything else.
  #
  # `rates` memoizes one lookup per currency across a page of tiles: the cache is Solid Cache,
  # so each lookup is a query, and this page renders eighty tiles off one or two currencies.
  #
  # nil when the rate is not cached and the caller won't fetch one — the index never makes a
  # live FX call, and in exactly that case the account total is sitting in its spinner too.
  def profit_in_usd(metrics, rates: {}, cache_only: true)
    return nil if metrics.blank?

    profit = (metrics[:total_amount_value_in_quote] || 0).to_d - (metrics[:total_quote_amount_invested] || 0).to_d
    return profit if profit.zero? # nothing is nothing in every currency — no rate needed

    currency = quote_asset&.symbol
    return nil if currency.nil?

    rate = (rates[currency] ||= Utilities::Currency.exchange_rate(from: currency, to: 'USD',
                                                                  cache_only: cache_only))
    return nil if rate.failure?

    profit * rate.data.to_d
  end

  # One tile's PnL, in both formats — the partial renders percent and USD amount side by side
  # and the page-level class picks one. Shared by every measurable bot type: they all replaced
  # the same target with the same partial.
  #
  # This runs in a background job, so it may fetch a rate; the index may not.
  #
  # NOT the account total. `User#global_pnl` walks every bot the user owns, and the dashboard
  # fires one of these jobs per bot — N bots did N x N bots' worth of work, ending in two live
  # FX conversions each (17.7s warm / 158s cold for fifteen bots). The total is owned by
  # User::BroadcastGlobalPnlUpdateJob, which the page requests once and which does not depend
  # on these jobs having run.
  def broadcast_pnl_update
    metrics_data = metrics_with_current_prices

    broadcast_replace_to(
      ["user_#{user_id}", :bot_updates],
      target: dom_id(self, :pnl),
      partial: 'bots/bot_tile/bot_tile_pnl',
      locals: { bot: self, pnl: metrics_data[:pnl], denomination: user.denomination,
                profit_usd: profit_in_usd(metrics_data, cache_only: false), loading: false }
    )
  end

  def broadcast_status_bar_update
    broadcast_replace_to(
      ["user_#{user_id}", :bot_updates],
      target: dom_id(self, :status_bar),
      partial: 'bots/status/status_bar',
      locals: { bot: self }
    )
  end

  def broadcast_status_button_update
    broadcast_replace_to(
      ["user_#{user_id}", :bot_updates],
      target: dom_id(self, :status_button),
      partial: 'bots/status/status_button',
      locals: { bot: self }
    )
  end

  def broadcast_columns_lock_update
    action = working? ? :add_class : :remove_class
    Turbo::StreamsChannel.broadcast_action_to(
      ["user_#{user_id}", :bot_updates],
      action:,
      target: dom_id(self, :columns),
      attributes: { 'class-name': 'bot-locked' }
    )
  end

  def broadcast_new_order(order)
    # TODO: When transactions point to real asset ids, we can use the asset ids directly instead of symbols
    ticker = order.exchange.tickers.find_by(base_asset: order.base_asset, quote_asset: order.quote_asset)
    decimals = {
      order.base_asset.symbol => ticker.base_decimals,
      order.quote_asset.symbol => ticker.quote_decimals
    }

    if transactions.limit(2).count == 1
      broadcast_remove_to(
        ["user_#{user_id}", :bot_updates],
        target: 'orders_list_placeholder'
      )
    end

    # Each transaction occupies two rows: the sentence row for the unified "All"
    # timeline (always, and it is also what the "Other" tab shows) and the columnar
    # row for the named tabs (submitted orders only) — mirroring show.turbo_stream.erb.
    # Prepend the timeline row first so the columnar row lands on top, matching the
    # initial-load order.
    broadcast_prepend_to(
      ["user_#{user_id}", :bot_updates],
      target: 'orders_list',
      partial: 'bots/orders/order_timeline',
      locals: { order:, decimals:, current_user: user }
    )

    if order.submitted?
      broadcast_prepend_to(
        ["user_#{user_id}", :bot_updates],
        target: 'orders_list',
        partial: 'bots/orders/order',
        locals: { order:, decimals:, current_user: user }
      )
    end

    broadcast_order_filters_update
  end

  def broadcast_order_filters_update
    broadcast_replace_to(
      ["user_#{user_id}", :bot_updates],
      target: dom_id(self, :order_filters),
      partial: 'bots/orders/order_filters',
      locals: { bot: self }
    )
  end

  def broadcast_updated_order(order)
    # TODO: When transactions point to real asset ids, we can use the asset ids directly instead of symbols
    ticker = order.exchange.tickers.find_by(base_asset: order.base_asset, quote_asset: order.quote_asset)
    decimals = {
      order.base_asset.symbol => ticker.base_decimals,
      order.quote_asset.symbol => ticker.quote_decimals
    }

    # Refresh both rows so the "All" timeline can't go stale (e.g. a filled order
    # still showing an "open" sentence + Cancel button) — see show.turbo_stream.erb.
    broadcast_replace_to(
      ["user_#{user_id}", :bot_updates],
      target: dom_id(order),
      partial: 'bots/orders/order',
      locals: { order:, decimals:, current_user: user }
    )

    broadcast_replace_to(
      ["user_#{user_id}", :bot_updates],
      target: dom_id(order, :timeline),
      partial: 'bots/orders/order_timeline',
      locals: { order:, decimals:, current_user: user }
    )

    broadcast_order_filters_update
  end

  # Override broadcast methods to use user's locale for translated partials
  def broadcast_replace_to(...)
    with_user_locale { super }
  end

  def broadcast_prepend_to(...)
    with_user_locale { super }
  end

  def broadcast_append_to(...)
    with_user_locale { super }
  end

  private

  def with_user_locale(&block)
    I18n.with_locale(user.locale || I18n.default_locale, &block)
  end

  def store_previous_exchange_id
    @previous_exchange_id = exchange_id_was
  end

  # A new bot goes to the end of the user's list. `0` is the unset sentinel rather than `nil`,
  # because the column is NOT NULL with a default of 0 — `position ||= …` would never fire.
  # The backfill migration starts at 1 for the same reason.
  def assign_position
    return if position.to_i.positive?

    self.position = user&.bots&.maximum(:position).to_i + 1
  end

  def custom_exchange_id_changed?
    exchange_id != @previous_exchange_id
  end
end
