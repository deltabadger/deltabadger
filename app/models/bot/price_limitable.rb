module Bot::PriceLimitable
  extend ActiveSupport::Concern

  PRICE_LIMIT_TIMING_CONDITIONS = %w[while after].freeze
  PRICE_LIMIT_VALUE_CONDITIONS = %w[above below between].freeze
  # Per-side trigger action. "pause" keeps today's gate behaviour; the flip action turns the
  # trigger into a direction switch (a buy-side trigger starts selling, a sell-side one starts
  # buying), making the bot a simple trading bot.
  PRICE_LIMIT_BUY_ACTIONS = %w[pause start_selling].freeze
  PRICE_LIMIT_SELL_ACTIONS = %w[pause start_buying].freeze
  PRICE_LIMIT_FLIP_ACTIONS = %w[start_selling start_buying].freeze

  included do
    store_accessor :settings,
                   :price_limited,
                   :price_limit,
                   :price_limit_range_lower_bound,
                   :price_limit_range_upper_bound,
                   :price_limit_timing_condition,
                   :price_limit_value_condition,
                   :price_limit_in_ticker_id,
                   :price_limit_action,
                   :sell_price_limited,
                   :sell_price_limit,
                   :sell_price_limit_range_lower_bound,
                   :sell_price_limit_range_upper_bound,
                   :sell_price_limit_timing_condition,
                   :sell_price_limit_value_condition,
                   :sell_price_limit_in_ticker_id,
                   :sell_price_limit_action
    store_accessor :transient_data,
                   :price_limit_enabled_at,
                   :price_limit_condition_met_at,
                   :sell_price_limit_enabled_at,
                   :sell_price_limit_condition_met_at

    after_initialize :initialize_price_limitable_settings

    before_save :set_price_limit_enabled_at, if: :will_save_change_to_settings?
    before_save :set_price_limit_condition_met_at, if: :will_save_change_to_settings?
    before_save :set_price_limit_in_ticker_id, if: :will_save_change_to_exchange_id?
    before_save :reset_price_limit_info_cache, if: :will_save_change_to_settings?
    before_save :set_price_limit_value_condition, if: :will_save_change_to_settings?

    validates :price_limited, inclusion: { in: [true, false] }
    validates :price_limit, numericality: { greater_than_or_equal_to: 0 }, if: :price_limited?
    validates :price_limit_range_lower_bound, numericality: { greater_than_or_equal_to: 0 }, if: :price_limited?
    validates :price_limit_range_upper_bound, numericality: { greater_than_or_equal_to: 0 }, if: :price_limited?
    validates :price_limit_timing_condition, inclusion: { in: PRICE_LIMIT_TIMING_CONDITIONS }
    validates :price_limit_value_condition, inclusion: { in: PRICE_LIMIT_VALUE_CONDITIONS }
    validates :price_limit_action, inclusion: { in: PRICE_LIMIT_BUY_ACTIONS }
    validates :sell_price_limit, numericality: { greater_than_or_equal_to: 0 }, if: :sell_price_limited?
    validates :sell_price_limit_range_lower_bound, numericality: { greater_than_or_equal_to: 0 }, if: :sell_price_limited?
    validates :sell_price_limit_range_upper_bound, numericality: { greater_than_or_equal_to: 0 }, if: :sell_price_limited?
    validates :sell_price_limit_action, inclusion: { in: PRICE_LIMIT_SELL_ACTIONS }

    decorators = Module.new do
      def parse_params(params)
        # timing_condition + action come from the merged …_mode select via expand_trigger_mode.
        super(params).merge(
          price_limited: params[:price_limited].presence&.in?(%w[1 true]),
          price_limit: params[:price_limit].presence&.to_f,
          price_limit_range_lower_bound: params[:price_limit_range_lower_bound].presence&.to_f,
          price_limit_range_upper_bound: params[:price_limit_range_upper_bound].presence&.to_f,
          price_limit_value_condition: params[:price_limit_value_condition].presence,
          price_limit_in_ticker_id: params[:price_limit_in_ticker_id].presence&.to_i,
          sell_price_limited: params[:sell_price_limited].presence&.in?(%w[1 true]),
          sell_price_limit: params[:sell_price_limit].presence&.to_f,
          sell_price_limit_range_lower_bound: params[:sell_price_limit_range_lower_bound].presence&.to_f,
          sell_price_limit_range_upper_bound: params[:sell_price_limit_range_upper_bound].presence&.to_f,
          sell_price_limit_value_condition: params[:sell_price_limit_value_condition].presence,
          sell_price_limit_in_ticker_id: params[:sell_price_limit_in_ticker_id].presence&.to_i
        ).compact.merge(expand_trigger_mode(params, 'price_limit', has_timing: true))
      end

      def execute_action
        return super unless active_price_limited?

        met = price_limit_condition_currently_met?
        if active_price_limit_flip?
          # A flip trigger only watches; it never pauses trading. When met, flip and break the
          # reschedule (no super → at most one flip per run; the fresh ActionJob runs the new side).
          return super unless met

          flip_direction!
          Result::Success.new({ break_reschedule: true })
        elsif met
          super
        else
          update!(status: :waiting)
          log_activity('limit_paused', details: { limit_type: :price })
          Bot::PriceLimitCheckJob.set(wait_until: Time.now.utc.end_of_minute).perform_later(self)
          Result::Success.new({ break_reschedule: true })
        end
      end

      def stop(stop_message_key: nil)
        is_stopped = super(stop_message_key:)
        return is_stopped unless price_limited? || sell_price_limited?

        cancel_scheduled_price_limit_check_jobs
        is_stopped
      end

      def started_at
        return super unless active_price_limited?

        condition_met_at = active_price_limit_condition_met_at
        if super.nil? || condition_met_at.nil?
          nil
        else
          [super, condition_met_at].max
        end
      end
    end

    prepend decorators
  end

  def price_limit_enabled_at
    value = super
    value.present? ? Time.zone.parse(value) : nil
  end

  def price_limit_condition_met_at
    value = super
    value.present? ? Time.zone.parse(value) : nil
  end

  def sell_price_limit_enabled_at
    value = super
    value.present? ? Time.zone.parse(value) : nil
  end

  def sell_price_limit_condition_met_at
    value = super
    value.present? ? Time.zone.parse(value) : nil
  end

  # Action reader fallbacks (default "pause", never persisted-on-load per invariant 1).
  def price_limit_action
    super.presence || 'pause'
  end

  def sell_price_limit_action
    super.presence || 'pause'
  end

  # Sell-side read-time fallbacks (never persisted-on-load, so an existing bot created before these
  # keys existed doesn't dirty settings and trip the Accountable guard — mirrors Bot::Reversible).
  # nil? (not presence) so a stored false/0 keeps its meaning.
  {
    sell_price_limited: false,
    sell_price_limit: 1_000_000,
    sell_price_limit_range_lower_bound: 0,
    sell_price_limit_range_upper_bound: 1_000_000,
    sell_price_limit_timing_condition: 'while',
    sell_price_limit_value_condition: 'above'
  }.each do |name, default|
    define_method(name) do
      value = super()
      value.nil? ? default : value
    end
  end

  def sell_price_limit_in_ticker_id
    super.presence || tickers.min_by { |t| t[:base] }&.id
  end

  def price_limited?
    price_limited == true
  end

  def sell_price_limited?
    sell_price_limited == true
  end

  # The active side's view of this trigger (picked by direction). Decorators read these.
  def active_price_limited?
    selling? ? sell_price_limited? : price_limited?
  end

  def active_price_limit_action
    selling? ? sell_price_limit_action : price_limit_action
  end

  def active_price_limit_flip?
    reversible? && PRICE_LIMIT_FLIP_ACTIONS.include?(active_price_limit_action)
  end

  def active_price_limit_condition_met_at
    selling? ? sell_price_limit_condition_met_at : price_limit_condition_met_at
  end

  # Evaluate the ACTIVE side's price condition, writing that side's condition_met_at. The check
  # job (Bot::PriceLimitCheckJob) polls this method unchanged for either direction.
  def get_price_limit_condition_met?
    return Result::Success.new(false) unless active_price_limited?
    return Result::Success.new(true) if price_limit_timing_condition_satisfied?

    ticker = tickers.available.find_by(id: public_send("#{price_limit_prefix}_in_ticker_id"))
    return Result::Success.new(false) unless ticker.present?

    result = ticker.get_last_price
    return result if result.failure?

    if price_limit_value_condition_satisfied?(result.data)
      if active_price_limit_condition_met_at.nil?
        update!("#{price_limit_prefix}_condition_met_at" => Time.current)
        broadcast_price_limit_info_update
      end
      Result::Success.new(true)
    else
      if active_price_limit_condition_met_at.present?
        set_missed_quote_amount
        update!("#{price_limit_prefix}_condition_met_at" => nil)
        broadcast_price_limit_info_update
      end
      Result::Success.new(false)
    end
  end

  def broadcast_price_limit_info_update
    ticker = tickers.available.find_by(id: public_send("#{price_limit_prefix}_in_ticker_id"))
    return unless ticker.present?

    price_result = ticker.get_last_price
    return if price_result.failure?
    return unless price_result.data.present?

    condition_met_result = get_price_limit_condition_met?
    return if condition_met_result.failure?

    info = Rails.cache.fetch(price_limit_info_cache_key, expires_in: 20.seconds) do
      {
        base: ticker.base_asset.symbol,
        quote: ticker.quote_asset.symbol,
        price: price_result.data.round(decimals[:quote]),
        condition_met: condition_met_result.data
      }
    end

    broadcast_replace_to(
      ["user_#{user_id}", :bot_updates],
      target: new_record? ? 'new-settings-price-limit-info' : 'settings-price-limit-info',
      partial: 'bots/settings/price_limit_info',
      locals: { bot: self, info: }
    )
  end

  def price_limit_info_from_cache
    Rails.cache.read(price_limit_info_cache_key)
  end

  private

  # "price_limit" while buying, "sell_price_limit" while selling — the key prefix for the active
  # side's mirror. Used by the read/eval path (not the per-side persistence callbacks).
  def price_limit_prefix
    selling? ? 'sell_price_limit' : 'price_limit'
  end

  def price_limit_condition_currently_met?
    result = get_price_limit_condition_met?
    result.success? && result.data
  end

  # Side-suffixed so a flip never renders the buy-side condition state from a stale cache entry.
  # selling? is defined on base Bot (false for non-reversible types).
  def price_limit_info_cache_key
    "bot_#{id}_price_limit_info_#{selling? ? 'selling' : 'buying'}"
  end

  def reset_price_limit_info_cache
    return if price_limit_was == price_limit &&
              price_limit_value_condition_was == price_limit_value_condition &&
              price_limit_in_ticker_id_was == price_limit_in_ticker_id &&
              price_limit_range_lower_bound_was == price_limit_range_lower_bound &&
              price_limit_range_upper_bound_was == price_limit_range_upper_bound &&
              sell_price_limit_was == sell_price_limit &&
              sell_price_limit_value_condition_was == sell_price_limit_value_condition &&
              sell_price_limit_in_ticker_id_was == sell_price_limit_in_ticker_id &&
              sell_price_limit_range_lower_bound_was == sell_price_limit_range_lower_bound &&
              sell_price_limit_range_upper_bound_was == sell_price_limit_range_upper_bound

    Rails.cache.delete(price_limit_info_cache_key)
  end

  def price_limit_timing_condition_satisfied?
    public_send("#{price_limit_prefix}_timing_condition") == 'after' &&
      active_price_limit_condition_met_at.present?
  end

  def price_limit_value_condition_satisfied?(current_price)
    limit = public_send(price_limit_prefix)
    lower = public_send("#{price_limit_prefix}_range_lower_bound")
    upper = public_send("#{price_limit_prefix}_range_upper_bound")
    case public_send("#{price_limit_prefix}_value_condition")
    when 'below'
      current_price < limit
    when 'above'
      current_price > limit
    when 'between'
      current_price >= [lower, upper].min && current_price <= [lower, upper].max
    else
      false
    end
  end

  def initialize_price_limitable_settings
    self.price_limited ||= false
    self.price_limit ||= 1_000_000 # 1 million meme
    self.price_limit_range_lower_bound ||= 0
    self.price_limit_range_upper_bound ||= 1_000_000 # 1 million meme
    self.price_limit_timing_condition ||= 'while'
    self.price_limit_value_condition ||= 'below'
    self.price_limit_in_ticker_id ||= tickers.min_by { |t| t[:base] }&.id
    # Sell-side defaults are read-time fallbacks (see readers above), never written on load.
  end

  def set_price_limit_enabled_at
    if price_limited_was != price_limited
      self.price_limit_enabled_at = price_limited? ? Time.current : nil
    end
    return if sell_price_limited_was == sell_price_limited

    self.sell_price_limit_enabled_at = sell_price_limited? ? Time.current : nil
  end

  def set_price_limit_condition_met_at
    self.price_limit_condition_met_at = nil if price_limited_was != price_limited
    self.sell_price_limit_condition_met_at = nil if sell_price_limited_was != sell_price_limited
  end

  def set_price_limit_in_ticker_id
    if price_limit_in_ticker_id_was.present? && exchange_id_was.present? && exchange_id_was != exchange_id
      ticker_was = Ticker.find_by(id: price_limit_in_ticker_id_was)
      self.price_limit_in_ticker_id = tickers.find_by(
        base_asset_id: ticker_was.base_asset_id,
        quote_asset_id: ticker_was.quote_asset_id
      )&.id
      sell_ticker_was = Ticker.find_by(id: sell_price_limit_in_ticker_id_was) if sell_price_limit_in_ticker_id_was.present?
      self.sell_price_limit_in_ticker_id = if sell_ticker_was
                                             tickers.find_by(base_asset_id: sell_ticker_was.base_asset_id,
                                                             quote_asset_id: sell_ticker_was.quote_asset_id)&.id
                                           else
                                             tickers.min_by { |t| t[:base] }&.id
                                           end
    else
      default_ticker_id = tickers.min_by { |t| t[:base] }&.id
      self.price_limit_in_ticker_id = default_ticker_id
      self.sell_price_limit_in_ticker_id = default_ticker_id
    end
  end

  def set_price_limit_value_condition
    set_buy_price_limit_value_condition
    set_sell_price_limit_value_condition
  end

  def set_buy_price_limit_value_condition
    return if price_limit_timing_condition_was == price_limit_timing_condition
    return if price_limit_timing_condition == 'while'

    self.price_limit_value_condition = 'above' if price_limit_value_condition == 'between'
  end

  def set_sell_price_limit_value_condition
    return if sell_price_limit_timing_condition_was == sell_price_limit_timing_condition
    return if sell_price_limit_timing_condition == 'while'

    self.sell_price_limit_value_condition = 'above' if sell_price_limit_value_condition == 'between'
  end

  def cancel_scheduled_price_limit_check_jobs
    cancel_solid_queue_jobs(
      job_class: 'Bot::PriceLimitCheckJob',
      record: self
    )
  end
end
