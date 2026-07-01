module Bot::PriceDropLimitable
  extend ActiveSupport::Concern

  # Buying watches a HIGH ("% drop from high"); selling is the mirror and watches a LOW ("% rise from
  # low"). All-time-low is degenerate for selling (price sits far above it → instant trigger), so the
  # sell side offers recent-low windows only. The union is kept for duration lookups by stored key.
  PRICE_DROP_LIMIT_BUY_TIME_WINDOW_CONDITIONS = {
    'ath' => Float::INFINITY,
    'twenty_four_hours' => 24.hours
  }.freeze
  PRICE_DROP_LIMIT_SELL_TIME_WINDOW_CONDITIONS = {
    'twenty_four_hours' => 24.hours,
    'seven_days' => 7.days
  }.freeze
  PRICE_DROP_LIMIT_TIME_WINDOW_CONDITIONS =
    PRICE_DROP_LIMIT_BUY_TIME_WINDOW_CONDITIONS.merge(PRICE_DROP_LIMIT_SELL_TIME_WINDOW_CONDITIONS).freeze

  # Per-side trigger action. "pause" keeps today's gate behaviour; the flip action turns the
  # trigger into a direction switch (a buy-side trigger starts selling, a sell-side one starts
  # buying), making the bot a simple trading bot.
  PRICE_DROP_LIMIT_BUY_ACTIONS = %w[pause start_selling].freeze
  PRICE_DROP_LIMIT_SELL_ACTIONS = %w[pause start_buying].freeze
  PRICE_DROP_LIMIT_FLIP_ACTIONS = %w[start_selling start_buying].freeze

  included do
    store_accessor :settings,
                   :price_drop_limited,
                   :price_drop_limit,
                   :price_drop_limit_time_window_condition,
                   :price_drop_limit_in_ticker_id,
                   :price_drop_limit_action,
                   :sell_price_drop_limited,
                   :sell_price_drop_limit,
                   :sell_price_drop_limit_time_window_condition,
                   :sell_price_drop_limit_in_ticker_id,
                   :sell_price_drop_limit_action
    store_accessor :transient_data,
                   :price_drop_limit_enabled_at,
                   :price_drop_limit_condition_met_at,
                   :sell_price_drop_limit_enabled_at,
                   :sell_price_drop_limit_condition_met_at

    after_initialize :initialize_price_drop_limitable_settings

    before_save :set_price_drop_limit_enabled_at, if: :will_save_change_to_settings?
    before_save :set_price_drop_limit_condition_met_at, if: :will_save_change_to_settings?
    before_save :set_price_drop_limit_in_ticker_id, if: :will_save_change_to_exchange_id?
    before_save :reset_price_drop_limit_info_cache, if: :will_save_change_to_settings?

    validates :price_drop_limited, inclusion: { in: [true, false] }
    validates :price_drop_limit,
              numericality: {
                greater_than_or_equal_to: 0,
                less_than_or_equal_to: 1
              },
              if: :price_drop_limited?
    validates :price_drop_limit_time_window_condition, inclusion: { in: PRICE_DROP_LIMIT_BUY_TIME_WINDOW_CONDITIONS.keys }
    validates :sell_price_drop_limit_time_window_condition, inclusion: { in: PRICE_DROP_LIMIT_SELL_TIME_WINDOW_CONDITIONS.keys }
    validates :price_drop_limit_action, inclusion: { in: PRICE_DROP_LIMIT_BUY_ACTIONS }
    validates :sell_price_drop_limit,
              numericality: {
                greater_than_or_equal_to: 0,
                less_than_or_equal_to: 1
              },
              if: :sell_price_drop_limited?
    validates :sell_price_drop_limit_action, inclusion: { in: PRICE_DROP_LIMIT_SELL_ACTIONS }

    decorators = Module.new do
      def parse_params(params)
        parsed_price_drop_limit = params[:price_drop_limit].presence&.to_f
        parsed_price_drop_limit = parsed_price_drop_limit.present? ? (parsed_price_drop_limit / 100).round(4) : nil
        parsed_sell_price_drop_limit = params[:sell_price_drop_limit].presence&.to_f
        parsed_sell_price_drop_limit = parsed_sell_price_drop_limit.present? ? (parsed_sell_price_drop_limit / 100).round(4) : nil
        # action comes from the merged …_mode select via expand_trigger_mode (no timing field here).
        super(params).merge(
          price_drop_limited: params[:price_drop_limited].presence&.in?(%w[1 true]),
          price_drop_limit: parsed_price_drop_limit,
          price_drop_limit_time_window_condition: params[:price_drop_limit_time_window_condition].presence,
          price_drop_limit_in_ticker_id: params[:price_drop_limit_in_ticker_id].presence&.to_i,
          sell_price_drop_limited: params[:sell_price_drop_limited].presence&.in?(%w[1 true]),
          sell_price_drop_limit: parsed_sell_price_drop_limit,
          sell_price_drop_limit_time_window_condition: params[:sell_price_drop_limit_time_window_condition].presence,
          sell_price_drop_limit_in_ticker_id: params[:sell_price_drop_limit_in_ticker_id].presence&.to_i
        ).compact.merge(expand_trigger_mode(params, 'price_drop_limit', has_timing: false))
      end

      def execute_action
        return super unless active_price_drop_limited?

        met = price_drop_limit_condition_currently_met?
        if active_price_drop_limit_flip?
          # A flip trigger only watches; it never pauses trading. When met, flip and break the
          # reschedule (no super → at most one flip per run; the fresh ActionJob runs the new side).
          return super unless met

          flip_direction!
          Result::Success.new({ break_reschedule: true })
        elsif met
          super
        else
          update!(status: :waiting)
          log_activity('limit_paused', details: { limit_type: :price_drop })
          Bot::PriceDropLimitCheckJob.set(wait_until: Time.now.utc.end_of_minute).perform_later(self)
          Result::Success.new({ break_reschedule: true })
        end
      end

      def stop(stop_message_key: nil)
        is_stopped = super(stop_message_key:)
        return is_stopped unless price_drop_limited? || sell_price_drop_limited?

        cancel_scheduled_price_drop_limit_check_jobs
        is_stopped
      end

      def started_at
        return super unless active_price_drop_limited?

        condition_met_at = active_price_drop_limit_condition_met_at
        if super.nil? || condition_met_at.nil?
          nil
        else
          [super, condition_met_at].max
        end
      end
    end

    prepend decorators
  end

  def price_drop_limit_enabled_at
    value = super
    value.present? ? Time.zone.parse(value) : nil
  end

  def price_drop_limit_condition_met_at
    value = super
    value.present? ? Time.zone.parse(value) : nil
  end

  def sell_price_drop_limit_enabled_at
    value = super
    value.present? ? Time.zone.parse(value) : nil
  end

  def sell_price_drop_limit_condition_met_at
    value = super
    value.present? ? Time.zone.parse(value) : nil
  end

  # Action reader fallbacks (default "pause", never persisted-on-load per invariant 1).
  def price_drop_limit_action
    super.presence || 'pause'
  end

  def sell_price_drop_limit_action
    super.presence || 'pause'
  end

  # Legacy/canonicalizing reader: an early build (and the prior initialize default) stored 'ath' on
  # the sell side, which the recent-low-only sell windows reject. Read it back as the 24h low without
  # dirtying settings (mirror of the direction/sell_interval reader fallbacks in reversible.rb). Also
  # the read-time default when unset.
  def sell_price_drop_limit_time_window_condition
    value = super
    value.nil? || value == 'ath' ? 'twenty_four_hours' : value
  end

  # Sell-side read-time fallbacks (never persisted-on-load — see price_limitable for the rationale).
  {
    sell_price_drop_limited: false,
    sell_price_drop_limit: 0.2
  }.each do |name, default|
    define_method(name) do
      value = super()
      value.nil? ? default : value
    end
  end

  def sell_price_drop_limit_in_ticker_id
    super.presence || tickers.min_by { |t| t[:base] }&.id
  end

  def price_drop_limited?
    price_drop_limited == true
  end

  def sell_price_drop_limited?
    sell_price_drop_limited == true
  end

  def price_drop_limit_time_window_duration
    PRICE_DROP_LIMIT_TIME_WINDOW_CONDITIONS[public_send("#{price_drop_limit_prefix}_time_window_condition")]
  end

  # The active side's view of this trigger (picked by direction). Decorators read these.
  def active_price_drop_limited?
    selling? ? sell_price_drop_limited? : price_drop_limited?
  end

  def active_price_drop_limit_action
    selling? ? sell_price_drop_limit_action : price_drop_limit_action
  end

  def active_price_drop_limit_flip?
    reversible? && PRICE_DROP_LIMIT_FLIP_ACTIONS.include?(active_price_drop_limit_action)
  end

  def active_price_drop_limit_condition_met_at
    selling? ? sell_price_drop_limit_condition_met_at : price_drop_limit_condition_met_at
  end

  # Evaluate the ACTIVE side's price-drop condition, writing that side's condition_met_at. The
  # check job (Bot::PriceDropLimitCheckJob) polls this method unchanged for either direction.
  def get_price_drop_limit_condition_met?
    return Result::Success.new(false) unless active_price_drop_limited?
    return Result::Success.new(true) if timing_condition_satisfied?

    ticker = tickers.available.find_by(id: public_send("#{price_drop_limit_prefix}_in_ticker_id"))
    return Result::Success.new(false) unless ticker.present?

    price_result = ticker.get_last_price
    return price_result if price_result.failure?

    reference_result = price_drop_reference_price_result(ticker)
    return reference_result if reference_result.failure?
    return Result::Success.new(false) unless reference_result.data.present?

    if price_drop_limit_time_window_condition_satisfied?(price_result.data, reference_result.data)
      if active_price_drop_limit_condition_met_at.nil?
        update!("#{price_drop_limit_prefix}_condition_met_at" => Time.current)
        broadcast_price_drop_limit_info_update
      end
      Result::Success.new(true)
    else
      if active_price_drop_limit_condition_met_at.present?
        set_missed_quote_amount
        update!("#{price_drop_limit_prefix}_condition_met_at" => nil)
        broadcast_price_drop_limit_info_update
      end
      Result::Success.new(false)
    end
  end

  def broadcast_price_drop_limit_info_update
    ticker = tickers.available.find_by(id: public_send("#{price_drop_limit_prefix}_in_ticker_id"))
    return unless ticker.present?

    reference_result = price_drop_reference_price_result(ticker)
    return if reference_result.failure?
    return unless reference_result.data.present?

    condition_met_result = get_price_drop_limit_condition_met?
    return if condition_met_result.failure?

    info = Rails.cache.fetch(price_drop_limit_info_cache_key, expires_in: 20.seconds) do
      {
        base: ticker.base_asset.symbol,
        quote: ticker.quote_asset.symbol,
        # The reference is the window high while buying and the window low while selling; the info
        # partial labels it accordingly.
        reference: reference_result.data,
        condition_met: condition_met_result.data
      }
    end

    broadcast_replace_to(
      ["user_#{user_id}", :bot_updates],
      target: new_record? ? 'new-settings-price-drop-limit-info' : 'settings-price-drop-limit-info',
      partial: 'bots/settings/price_drop_limit_info',
      locals: { bot: self, info: }
    )
  end

  def price_drop_limit_info_from_cache
    Rails.cache.read(price_drop_limit_info_cache_key)
  end

  private

  # "price_drop_limit" while buying, "sell_price_drop_limit" while selling — the key prefix for
  # the active side's mirror. Used by the read/eval path (not the per-side persistence callbacks).
  def price_drop_limit_prefix
    selling? ? 'sell_price_drop_limit' : 'price_drop_limit'
  end

  def price_drop_limit_condition_currently_met?
    result = get_price_drop_limit_condition_met?
    result.success? && result.data
  end

  # Side-suffixed so a flip never renders the buy-side high as the sell-side low (or vice versa)
  # from a stale cache entry. selling? is defined on base Bot (false for non-reversible types).
  def price_drop_limit_info_cache_key
    "bot_#{id}_price_drop_limit_info_#{selling? ? 'selling' : 'buying'}"
  end

  # Buying watches the window HIGH (drop-from-high); selling watches the window LOW (rise-from-low).
  def price_drop_reference_price_result(ticker)
    if selling?
      ticker.get_low_of_last(duration: price_drop_limit_time_window_duration)
    else
      ticker.get_high_of_last(duration: price_drop_limit_time_window_duration)
    end
  end

  def reset_price_drop_limit_info_cache
    return if price_drop_limit_was == price_drop_limit &&
              price_drop_limit_time_window_condition_was == price_drop_limit_time_window_condition &&
              price_drop_limit_in_ticker_id_was == price_drop_limit_in_ticker_id &&
              sell_price_drop_limit_was == sell_price_drop_limit &&
              sell_price_drop_limit_time_window_condition_was == sell_price_drop_limit_time_window_condition &&
              sell_price_drop_limit_in_ticker_id_was == sell_price_drop_limit_in_ticker_id

    Rails.cache.delete(price_drop_limit_info_cache_key)
  end

  def timing_condition_satisfied?
    active_price_drop_limit_condition_met_at.present?
  end

  def price_drop_limit_time_window_condition_satisfied?(current_price, reference_price)
    # price_drop_limit_prefix is already the full attribute name (price_drop_limit /
    # sell_price_drop_limit), so we read it directly without appending "_limit".
    limit = public_send(price_drop_limit_prefix)
    if selling?
      current_price > (1 + limit) * reference_price # rise from the recent low
    else
      current_price < (1 - limit) * reference_price # drop from the high
    end
  end

  def initialize_price_drop_limitable_settings
    self.price_drop_limited ||= false
    self.price_drop_limit ||= 0.2
    self.price_drop_limit_time_window_condition ||= 'ath'
    self.price_drop_limit_in_ticker_id ||= tickers.min_by { |t| t[:base] }&.id
    # Sell-side defaults are read-time fallbacks (see readers above), never written on load.
  end

  def set_price_drop_limit_enabled_at
    if price_drop_limited_was != price_drop_limited
      self.price_drop_limit_enabled_at = price_drop_limited? ? Time.current : nil
    end
    return if sell_price_drop_limited_was == sell_price_drop_limited

    self.sell_price_drop_limit_enabled_at = sell_price_drop_limited? ? Time.current : nil
  end

  def set_price_drop_limit_condition_met_at
    self.price_drop_limit_condition_met_at = nil if price_drop_limited_was != price_drop_limited
    self.sell_price_drop_limit_condition_met_at = nil if sell_price_drop_limited_was != sell_price_drop_limited
  end

  def set_price_drop_limit_in_ticker_id
    if price_drop_limit_in_ticker_id_was.present? && exchange_id_was.present? && exchange_id_was != exchange_id
      ticker_was = Ticker.find_by(id: price_drop_limit_in_ticker_id_was)
      self.price_drop_limit_in_ticker_id = tickers.find_by(
        base_asset_id: ticker_was.base_asset_id,
        quote_asset_id: ticker_was.quote_asset_id
      )&.id
      sell_ticker_was = Ticker.find_by(id: sell_price_drop_limit_in_ticker_id_was) if sell_price_drop_limit_in_ticker_id_was.present?
      self.sell_price_drop_limit_in_ticker_id = if sell_ticker_was
                                                  tickers.find_by(base_asset_id: sell_ticker_was.base_asset_id,
                                                                  quote_asset_id: sell_ticker_was.quote_asset_id)&.id
                                                else
                                                  tickers.min_by { |t| t[:base] }&.id
                                                end
    else
      default_ticker_id = tickers.min_by { |t| t[:base] }&.id
      self.price_drop_limit_in_ticker_id = default_ticker_id
      self.sell_price_drop_limit_in_ticker_id = default_ticker_id
    end
  end

  def cancel_scheduled_price_drop_limit_check_jobs
    cancel_solid_queue_jobs(
      job_class: 'Bot::PriceDropLimitCheckJob',
      record: self
    )
  end
end
