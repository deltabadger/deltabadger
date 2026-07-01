# The sell-side mirror of Bot::QuoteAmountLimitable: "Don't sell more than N <base>". Only
# DcaSingleAsset (reversible) includes it, and every path is gated to selling? so it stays inert
# while the bot is buying. Accounting is denominated in BASE and counts open sells too, so lagging
# confirmations can never oversell past the cap.
module Bot::BaseAmountLimitable
  extend ActiveSupport::Concern

  included do
    store_accessor :settings,
                   :base_amount_limited,
                   :base_amount_limit
    store_accessor :transient_data,
                   :base_amount_limit_enabled_at

    before_save :set_base_amount_limit_enabled_at, if: :will_save_change_to_settings?

    validates :base_amount_limited, inclusion: { in: [true, false] }
    validates :base_amount_limit,
              numericality: { greater_than: 0 },
              if: -> { base_amount_limited? && selling? }
    # Parity with the quote cap: don't let an already-exhausted cap (re)start the bot into an active
    # state where every sell tick is a no-op.
    validate :validate_base_amount_limit_not_reached, if: -> { base_amount_limited? && selling? }, on: :start

    decorators = Module.new do
      def parse_params(params)
        super(params).merge(
          base_amount_limited: params[:base_amount_limited].presence&.in?(%w[1 true]),
          base_amount_limit: params[:base_amount_limit].presence&.to_f
        ).compact
      end
    end

    prepend decorators
  end

  def base_amount_limit_enabled_at
    value = super
    value.present? ? Time.zone.parse(value) : nil
  end

  # Read-time fallback (never persisted-on-load, so an existing bot without this key doesn't dirty
  # settings — the inclusion validation reads through this to a valid false).
  def base_amount_limited
    value = super
    value.nil? ? false : value
  end

  def base_amount_limited?
    base_amount_limited == true
  end

  # Base sold since the cap was enabled: executed base of CLOSED sells plus the requested base of
  # still-OPEN sells (open orders are reserved so confirmations lagging can't oversell the cap).
  def base_amount_available_before_limit_reached
    return Float::INFINITY unless base_amount_limited?
    return Float::INFINITY if base_amount_limit.blank?

    # Closed rows fall back to the requested `amount` when amount_exec was never backfilled — matching
    # Bot#total_amount and the metrics, so a nil-exec close can't silently regain cap allowance.
    closed_base = transactions.submitted.sell
                              .where('created_at >= ?', base_amount_limit_enabled_at)
                              .closed
                              .pluck(Arel.sql('COALESCE(amount_exec, amount)')).compact.sum

    open_base = transactions.submitted.sell
                            .where('created_at >= ?', base_amount_limit_enabled_at)
                            .waiting
                            .pluck(:amount).compact.sum

    [base_amount_limit.to_d - (closed_base + open_base), 0].max
  end

  def base_amount_limit_reached?
    return false unless base_amount_limited?

    base_amount_available_before_limit_reached < minimum_base_amount_limit
  end

  def minimum_base_amount_limit
    least_precise_base_decimals = tickers.pluck(:base_decimals).compact.min
    return 0 if least_precise_base_decimals.nil?

    1.0 / (10**least_precise_base_decimals)
  end

  def handle_base_amount_limit_update
    return unless base_amount_limited? && selling?

    broadcast_base_amount_limit_update
    return unless base_amount_limit_reached?

    Bot::StopJob.perform_later(self, stop_message_key: 'bot.settings.extra_amount_limit.amount_sold')
    notify_stopped_by_base_amount_limit
  end

  private

  def set_base_amount_limit_enabled_at
    return if base_amount_limited_was == base_amount_limited

    self.base_amount_limit_enabled_at = base_amount_limited? ? Time.current : nil
  end

  def validate_base_amount_limit_not_reached
    errors.add(:settings, :base_amount_limit_reached) if base_amount_limit_reached?
  end

  def broadcast_base_amount_limit_update
    broadcast_replace_to(
      ["user_#{user_id}", :bot_updates],
      target: 'settings-amount-limit-info',
      partial: 'bots/settings/amount_limit_info',
      locals: { bot: self }
    )
  end
end
