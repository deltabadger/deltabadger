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

  belongs_to :user
  has_many :transactions, dependent: :destroy

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
    # timeline (always) and the columnar row for the named tabs (submitted orders
    # only) — mirroring show.turbo_stream.erb. Prepend the timeline row first so the
    # columnar row lands on top, matching the initial-load order.
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
        locals: { order:, decimals:, exchange_name: order.exchange.name, current_user: user, fetch: false }
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
      locals: { order:, decimals:, exchange_name: order.exchange.name, current_user: user, fetch: false }
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

  def custom_exchange_id_changed?
    exchange_id != @previous_exchange_id
  end
end
