# Threshold-driven portfolio rebalancing, independent of the DCA schedule.
#
# The DCA leg is schedule-driven: Automation::Schedulable arms a Bot::ActionJob that reschedules
# itself, and every trigger concern (PriceDropLimitable & co.) only decorates that tick — it can
# veto or redirect a tick the clock summoned, never cause one. Rebalancing is the other shape: it
# has no clock at all. Bot::EvaluateRebalancersJob polls, this asks "how far has the split drifted",
# and Bot::RebalanceJob acts. That is why it fires while the DCA leg is stopped.
#
# Rebalancing is a SWAP, not a contribution: sell the overweight asset, buy the underweight one with
# the proceeds, no new money. Between those two legs the bot holds quote cash that #metrics does not
# model, so the drift reading in that window is wrong — base0 has shrunk and the uncounted cash makes
# it look underweight, which a naive next evaluation would "fix" by selling base1. That churn loop
# bleeds fees and realizes losses, so a pending rebalance blocks a new one absolutely: while state
# exists the only legal move is to finish it.
module Bot::Rebalanceable
  extend ActiveSupport::Concern

  # Included here rather than per bot type so the stand-down guards below can always ask. The
  # dual-asset bot can never have a quitter to liquidate, but it must still answer the question.
  include Bot::LiquidationState

  PENDING_KEY = 'rebalance_pending'.freeze

  PHASE_SELLING = 'selling'.freeze
  PHASE_BUYING = 'buying'.freeze
  # Terminal halt, never a retry state. Reached when a placement's outcome is UNKNOWN — see
  # Bot::RebalanceJob for why an unknown outcome can never be replayed automatically.
  PHASE_AMBIGUOUS = 'ambiguous'.freeze

  DEFAULT_THRESHOLD = '0.05'.to_d

  included do
    store_accessor :settings, :rebalance_enabled, :rebalance_threshold

    # Validates the EFFECTIVE value (the reader never returns nil), so an existing row that predates
    # this feature stays valid while a corrupted stored value is still rejected.
    validates :rebalance_threshold, numericality: { greater_than: 0, less_than_or_equal_to: 1 }

    # Prepended so this parse_params decorator is outermost: the inner decorators each .compact
    # their result, which would strip a deliberate "unchecked" false back to nothing.
    prepend(Module.new do
      # The DCA leg stands down while a rebalance is mid-flight. The per-exchange semaphore prevents
      # the two legs OVERLAPPING, not one following the other: a DCA tick landing between the
      # rebalance's sell and buy would spend the sale proceeds on its own order, leaving the owed buy
      # underfunded. Lives here rather than in each bot type so a type cannot forget it — the index
      # bot did exactly that when this guard was per-class.
      #
      # The contribution is not lost: missed_quote_amount carries it into the next tick.
      def execute_action
        if rebalance_pending?
          log_activity('dca_skipped_rebalance_pending', level: :info)
          return Result::Success.new
        end
        # A resting liquidation sell is a live claim on those coins, and a quitter can re-enter the
        # index while it rests — so without this the DCA leg could buy the very asset the bot is
        # selling.
        #
        # liquidation_blocks_trading? SWEEPS AND PROMOTES before answering, and that is the whole
        # point of asking it here rather than reading the state directly. This decorator is prepended
        # ahead of Bot::LimitOrderable, so returning early skips the open-order sweep that would
        # otherwise advance the very order we are waiting on — the bot would stand down forever on a
        # row nothing ever polls. Same for a `placing` intent left by a dead worker: the clock is the
        # reliable trigger for surfacing it, not the user happening to open the widget.
        if liquidation_blocks_trading?
          log_activity('dca_skipped_liquidation_pending', level: :info)
          return Result::Success.new
        end

        super
      end

      def parse_params(params)
        result = super
        return result unless params.respond_to?(:key?)

        result[:rebalance_enabled] = params[:rebalance_enabled].presence&.in?(%w[1 true]) || false if params.key?(:rebalance_enabled)
        if params.key?(:rebalance_threshold)
          value = params[:rebalance_threshold].presence&.to_f
          result[:rebalance_threshold] = value.present? ? (value / 100).round(4) : nil
        end
        result
      end
    end)
  end

  # Reader fallbacks, never persisted defaults (the Bot::Reversible pattern). A row created before
  # this feature has no such key, and writing one on load would dirty `settings` and trip
  # Accountable#check_missed_quote_amount_was_set on the next routine save.
  def rebalance_enabled?
    ActiveModel::Type::Boolean.new.cast(rebalance_enabled).present?
  end

  def rebalance_threshold
    value = super
    value.presence&.to_d || DEFAULT_THRESHOLD
  end

  # How far the portfolio has drifted from its targets, as a fraction (0.2 == 20 points off): the
  # LARGEST deviation any single asset shows. For two assets the two deviations are equal and
  # opposite, so this is exactly the old |value0/total - allocation0|; for an index it is the one
  # asset furthest from where it should be, which is what "an asset drifted more than X" says.
  #
  # Measured on LIVE value, never on cost basis: an asset that doubled is overweight regardless of
  # what it cost. nil when there is nothing to measure (no holdings) or nothing trustworthy to
  # measure it with (stale prices) — see each type's #rebalance_targets.
  def rebalance_drift
    entries = rebalance_targets
    return nil if entries.blank?

    total = entries.sum { |entry| entry[:value].to_d }
    return nil unless total.positive?

    entries.map { |entry| ((entry[:value].to_d / total) - entry[:target].to_d).abs }.max
  end

  # [{ ticker:, value: <in quote>, target: <weight 0..1> }, ...], or nil when the portfolio cannot
  # be valued right now. The ONE thing a bot type has to supply — drift, which asset to sell and
  # which to buy all follow from it (see Bot::Rebalancer).
  def rebalance_targets
    raise NotImplementedError, "#{self.class.name} must implement rebalance_targets"
  end

  def rebalance_due?
    return false unless rebalance_enabled?
    return false if rebalance_pending?
    # Same reason the DCA leg stands down: a resting liquidation sell must not be bought against.
    # Guards NEW work only — Bot::Rebalancer#rebalance! resumes a swap already mid-flight regardless,
    # because that one owes its buy.
    return false if liquidation_blocks_trading?

    drift = rebalance_drift
    drift.present? && drift > rebalance_threshold
  end

  # --- pending state ------------------------------------------------------------------------
  # Lives in transient_data (not settings) so writing it never touches the settings-dirty guard.
  # remaining_quote_amount is the durable ledger of cash the rebalance still owes the buy leg: a
  # partially filled then cancelled buy cannot be reconstructed from transaction rows after a crash.

  def rebalance_pending
    value = transient_data[PENDING_KEY]
    value.is_a?(Hash) ? value.symbolize_keys : nil
  end

  def rebalance_pending?
    rebalance_pending.present?
  end

  def rebalance_ambiguous?
    rebalance_pending&.dig(:phase) == PHASE_AMBIGUOUS
  end

  # BigDecimal is stored as a String on purpose: a BigDecimal put into a JSON column round-trips
  # back as a String anyway, and a consumer expecting a number then divides by one and 500s.
  def rebalance_remaining_quote_amount
    rebalance_pending&.dig(:remaining_quote_amount)&.to_d
  end

  # buy_attempted separates "the handoff to the buy leg never reached the network" from "a buy was
  # sent and we lost track of it". Only the first is safe to replay; the second may already be live.
  def set_rebalance_pending!(phase:, sell_transaction_id: nil, buy_transaction_id: nil,
                             remaining_quote_amount: nil, buy_attempted: false)
    payload = {
      'phase' => phase,
      'sell_transaction_id' => sell_transaction_id,
      'buy_transaction_id' => buy_transaction_id,
      'remaining_quote_amount' => remaining_quote_amount&.to_d&.to_s('F'),
      'buy_attempted' => buy_attempted.present?
    }
    # update_columns, matching the other transient_data writes (Bot::ActionJob, Bot::Reversible):
    # this is bookkeeping, not a user edit, and must not run validations or dirty settings.
    update_columns(transient_data: transient_data.merge(PENDING_KEY => payload))
  end

  def clear_rebalance_pending!
    update_columns(transient_data: transient_data.except(PENDING_KEY))
  end

  # --- "drifted, but no trade is big enough" -------------------------------------------------
  # With two assets, a correction below the venue minimum just means the drift is tiny, so staying
  # silent is right. With more, the drift that TRIPS the band and the trade that FIXES it are
  # different numbers: one asset can be 8 points light while the excess sits spread across the other
  # forty-nine, each a rounding error. The largest single sale is then below the floor and nothing
  # ever happens — while the widget paints the breached drift green, meaning "criteria met".
  #
  # A flag rather than an activity log: this repeats every poll for as long as it lasts, so a log
  # entry would be spam. Cleared as soon as a rebalance places anything.
  BELOW_MINIMUM_KEY = 'rebalance_below_minimum_at'.freeze

  def rebalance_below_minimum?
    transient_data[BELOW_MINIMUM_KEY].present?
  end

  def flag_rebalance_below_minimum!
    return if rebalance_below_minimum?

    update_columns(transient_data: transient_data.merge(BELOW_MINIMUM_KEY => Time.current.iso8601))
  end

  def clear_rebalance_below_minimum!
    return unless rebalance_below_minimum?

    update_columns(transient_data: transient_data.except(BELOW_MINIMUM_KEY))
  end
end
