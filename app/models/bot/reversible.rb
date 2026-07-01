module Bot::Reversible
  extend ActiveSupport::Concern

  DIRECTIONS = %w[buying selling].freeze

  included do
    store_accessor :settings, :direction, :sell_amount, :sell_interval

    validates :direction, inclusion: { in: DIRECTIONS }, allow_nil: true
    # Blank stays valid (sell sentence not filled yet — a no-op skip); reject negative/garbage so a
    # bad value can't silently disable selling.
    validates :sell_amount, numericality: { greater_than: 0 }, allow_nil: true
    # The reader falls back to a valid buy interval, so this validates the effective value and
    # rejects only a corrupted stored value (which would crash scheduling on the flip).
    validates :sell_interval, inclusion: { in: Automation::Schedulable::INTERVALS.keys }
    validate :validate_unchangeable_sell_interval, on: :update

    # Prepend (after SmartIntervalable, so this sits above its decorator) so the sell cadence wins
    # while selling. Without Smart Intervals it's just the sell interval; with it, the interval is
    # subdivided by the base split (sell_amount / smart_interval_base_amount) — the mirror of the
    # buy-side quote split. Fall back to the buy cadence (super) if a stored value is unusable.
    prepend(Module.new do
      def effective_interval_duration
        return super unless selling?

        base_duration = Automation::Schedulable::INTERVALS[sell_interval] || super
        if smart_intervaled? && smart_interval_base_amount.present? && sell_amount.present? && sell_amount.positive?
          # .seconds re-conversion required (see SmartIntervalable) so Time + duration keeps working.
          return (base_duration / (sell_amount.to_f / smart_interval_base_amount)).seconds
        end

        base_duration
      end

      # Outermost parse_params decorator (Reversible is included last). sell_amount is blankable — a
      # SUBMITTED blank is an explicit clear (nil) back to the no-op state. The inner decorators each
      # .compact their result, stripping a nil, so re-apply the clear here after they run. A form that
      # did not submit the field leaves it untouched.
      def parse_params(params)
        result = super
        result[:sell_amount] = params[:sell_amount].presence&.to_f if params.respond_to?(:key?) && params.key?(:sell_amount)
        result
      end
    end)
  end

  # Reader fallback so the default is never written into settings. An existing row
  # created before this feature has no "direction" key; reading it must not dirty
  # settings, or the next routine save would trip
  # Accountable#check_missed_quote_amount_was_set.
  def direction
    super.presence || 'buying'
  end

  # Sell-side config. Both use reader fallbacks (not persisted-on-load) per invariant 1.
  # The sell cadence inherits the buy interval until the user picks its own.
  def sell_interval
    super.presence || interval
  end

  # Stays blank until the user fills the sell sentence; a blank/zero amount makes the sell
  # tick a no-op skip (nothing to sell yet). Coerced to BigDecimal for the cap math.
  def sell_amount
    value = super
    value.presence&.to_d
  end

  def buying?
    direction == 'buying'
  end

  def selling?
    direction == 'selling'
  end

  def reversible?
    true
  end

  # The single choke point for both the manual ⇄ flip and (later) the trigger flips.
  # Persisting settings requires set_missed_quote_amount first (Accountable guard).
  #
  # An INACTIVE bot (created / stopped) only changes its stored direction — reversing must never
  # start a bot or schedule a trade outside the normal start lifecycle. It will run the new way
  # when the user next starts it.
  #
  # A WORKING bot keeps running but on the new side, so flip_direction! owns the reschedule (the
  # ActionJob does not reschedule on a break_reschedule path):
  #   1. Reset the cadence anchor (started_at) so the new side starts fresh rather than inheriting
  #      the other side's next-run timestamp.
  #   2. Cancel the stale Bot::ActionJob AND any pending limit-check jobs (a limit-paused bot has a
  #      scheduled Bot::*LimitCheckJob that would resume it and race the fresh action job), then
  #      enqueue a fresh Bot::ActionJob at the new cadence.
  #   3. Re-broadcast via Bot::BroadcastAfterScheduledActionJob.
  # Pure mechanism — the manual path (controller) gates on flip_blocked_by_inflight_job? first;
  # trigger flips (M5) call this directly from inside the running (working) ActionJob.
  def flip_direction!
    leaving_buy_side = buying?
    set_missed_quote_amount
    cancelled_buy_reserve = cancel_unfilled_orders
    # A cancelled unfilled buy no longer catches up the schedule, but set_missed_quote_amount already
    # counted its reserved quote as invested (deflating the carry). Restore that reserve so a flip-back
    # to buying doesn't under-buy — bounded by effective_quote_amount when the update! below saves.
    if leaving_buy_side && cancelled_buy_reserve.positive?
      self.missed_quote_amount = [missed_quote_amount + cancelled_buy_reserve, effective_quote_amount].min
    end
    flipped = (selling? ? 'buying' : 'selling')

    unless working?
      update!(direction: flipped)
      reset_trigger_condition_timestamps
      return
    end

    update!(direction: flipped, status: :scheduled, started_at: Time.current)
    cancel_scheduled_action_jobs
    cancel_scheduled_limit_check_jobs
    reset_trigger_condition_timestamps
    Bot::ActionJob.set(wait_until: next_interval_checkpoint_at).perform_later(self)
    Bot::BroadcastAfterScheduledActionJob.perform_later(self)
  end

  # Manual-flip deferral guard. cancel_scheduled_* (step 3 above) cannot cancel a job that
  # a worker has already CLAIMED, so the manual path must defer while one is in flight:
  #   (a) a claimed Bot::ActionJob could place an old-direction order or reschedule with
  #       stale state — this catches the window where the worker has claimed the job but not
  #       yet flipped status to :executing, which a status-only check would miss;
  #   (b) a claimed Bot::*LimitCheckJob could enqueue an ActionJob that races the fresh one.
  # Claimed-only (narrower than active_limit_check_job?, which also counts merely-scheduled
  # jobs and would over-block the common limit-paused case).
  def flip_blocked_by_inflight_job?
    return false unless defined?(SolidQueue)

    inflight_classes = [Bot::ActionJob, *Bot::LimitCheckable::LIMIT_CHECK_JOBS.values.map(&:first)].map(&:name)
    global_id = to_global_id.to_s
    SolidQueue::ClaimedExecution.joins(:job)
                                .where(solid_queue_jobs: { class_name: inflight_classes })
                                .any? { |execution| job_matches_record?(execution.job, global_id) }
  end

  private

  # Cancel every still-open order on reversal — they belong to the side we are leaving, and an
  # unfilled buy must not be counted as accumulated base on the new sell side. Best-effort: a failed
  # cancel (e.g. the order just filled, or an exchange hiccup) is logged, not fatal, so a flip never
  # crashes. Transaction#cancel hits the exchange and enqueues a re-fetch that marks the row cancelled.
  # Returns the total reserved quote of successfully-cancelled BUY orders, so flip_direction! can
  # restore it to the buy carry (a cancelled unfilled buy never executed its catch-up). Failed cancels
  # are NOT counted — that order may still be live on the exchange.
  def cancel_unfilled_orders
    cancelled_buy_reserve = 0.to_d
    transactions.waiting.find_each do |order|
      result = order.cancel
      if result.success?
        if order.buy?
          requested = order.quote_amount || (order.amount * order.price)
          cancelled_buy_reserve += [requested.to_d - order.quote_amount_exec.to_d, 0.to_d].max
        end
        next
      end

      # A dangling order left on the leaving side is operationally relevant — log it AND record a
      # user-visible activity event (not just a buried warn line).
      Rails.logger.warn("flip_direction! could not cancel order #{order.external_id} for bot #{id}: #{result.errors.to_sentence}")
      log_activity('order_cancel_failed',
                   "Could not cancel order #{order.external_id} while reversing — it may still be open on the exchange.",
                   level: :warning, details: { order_id: order.external_id })
    end
    cancelled_buy_reserve
  end

  # Parity with validate_unchangeable_interval: the sell cadence is fixed while it is the
  # ACTIVE side (working + selling). It stays freely editable while buying (inactive side).
  # Compares the raw stored values to sidestep the sell_interval reader fallback.
  def validate_unchangeable_sell_interval
    return unless settings_changed?
    return unless working? && selling?
    return unless settings_was['sell_interval'] != settings['sell_interval']

    errors.add(:settings, :unchangeable_sell_interval,
               message: 'Sell interval cannot be changed while the bot is selling')
  end

  # On a flip, clear every trigger's *_condition_met_at (both sides) so a stale "after"-timing
  # timestamp from the side we are leaving can't leak into the side we are entering. These live in
  # transient_data, so update_columns is safe (no settings change, no Accountable guard). The new
  # direction re-detects its own condition fresh on the next tick.
  def reset_trigger_condition_timestamps
    met_keys = transient_data.keys.grep(/_condition_met_at\z/)
    return if met_keys.none? { |k| transient_data[k].present? }

    update_columns(transient_data: transient_data.merge(met_keys.index_with { nil }))
  end

  # Cancel the scheduled/ready limit-check job for every trigger type. Harmless no-op for
  # types with no pending job; only the live type actually has one.
  def cancel_scheduled_limit_check_jobs
    cancel_scheduled_price_limit_check_jobs
    cancel_scheduled_price_drop_limit_check_jobs
    cancel_scheduled_moving_average_limit_check_jobs
    cancel_scheduled_indicator_limit_check_jobs
  end
end
