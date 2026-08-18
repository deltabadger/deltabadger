# Shared "limit-check chain" introspection used by recovery (RepairOrphanedBotsJob) and the
# one-time rescue.
#
# WHICH chain is live? NOT derivable from *_limited? predicates: a bot can enable several limit
# types, and execute_action's decorators pause on the FIRST type whose condition is UNMET (an
# outer enabled-but-satisfied limit calls super and passes through). The authoritative record of
# which type actually paused is the durable `limit_paused` activity log each decorator writes with
# details: { limit_type: <type> } right before enqueuing its check job. We read the most recent one.
module Bot::LimitCheckable
  extend ActiveSupport::Concern

  # limit_type (as stored in the limit_paused log) → [check-job class, next-check-time lambda].
  # Mirrors each *_limitable.rb execute_action decorator's enqueue.
  LIMIT_CHECK_JOBS = {
    'price' => [Bot::PriceLimitCheckJob, ->(_bot) { Time.now.utc.end_of_minute }],
    'price_drop' => [Bot::PriceDropLimitCheckJob, ->(_bot) { Time.now.utc.end_of_minute }],
    'moving_average' => [Bot::MovingAverageLimitCheckJob,
                         ->(bot) { Time.now.utc + Utilities::Time.seconds_to_current_candle_close(bot.moving_average_limit_in_timeframe_duration) }],
    'indicator' => [Bot::IndicatorLimitCheckJob,
                    ->(bot) { Time.now.utc + Utilities::Time.seconds_to_current_candle_close(bot.indicator_limit_in_timeframe_duration) }]
  }.freeze

  # A failed condition read is NOT "the condition is not met". Every gate below used to flatten a
  # Result::Failure to false, which reads as "the market has not reached your price" — so a bot whose
  # key the venue rejects parks itself, tells the user it is waiting for a price, polls forever and
  # never trades again, with no error and no email. A credential rejection is the same answer on
  # every tick, so it must escape; every other failure keeps the old conservative behaviour (wait —
  # never trade on a read we could not make), which the check jobs already reschedule.
  def limit_condition_met?(result)
    exchange.raise_on_invalid_key!(result)

    result.success? && result.data
  end

  # The limit type CURRENTLY pausing this bot, or nil. "Currently" — not "ever": a bot keeps its
  # limit_paused logs forever, so we must distinguish a bot resting on a live limit pause from one
  # that limit-paused in the past but has since run a normal cycle. Signal: the latest limit_paused
  # must be from the most-recent ActionJob run, i.e. at/after `last_action_job_at` (ActionJob sets
  # that at app/jobs/bot/action_job.rb:64, immediately BEFORE execute_action writes the pause log,
  # so the pause is always a hair LATER than last_action_job_at on the pausing run). A subsequent
  # normal cycle advances last_action_job_at past the old pause → not currently limited.
  def live_limit_check_type
    log = bot_activity_logs.where(event: 'limit_paused').order(created_at: :desc, id: :desc).first
    return nil unless log

    last_run = last_action_job_at
    return nil if last_run.present? && log.created_at < last_run

    log.details&.fetch('limit_type', nil)&.to_s
  end

  # True when this bot currently rests on a limit-check chain (has a recorded paused type).
  def limited?
    live_limit_check_type.present?
  end

  def live_limit_check_job_class
    LIMIT_CHECK_JOBS[live_limit_check_type]&.first
  end

  # Is there a live (not dead-lettered) limit-check job for this bot's live type? Checks every
  # ACTIVE Solid Queue execution state — Scheduled, Ready, Claimed (mid-run), Blocked (concurrency-
  # gated) — so recovery never double-enqueues a still-alive chain. Deliberately EXCLUDES
  # FailedExecution: a job that exists only there is the dead chain we recover.
  def pending_limit_check_job?
    klass = live_limit_check_job_class
    return false unless klass

    active_job?(job_class: klass.name, record: self)
  end

  # Re-enqueue the live check job at its type-specific next check time. No-op if no paused type.
  def enqueue_limit_check_job
    type = live_limit_check_type
    entry = LIMIT_CHECK_JOBS[type]
    return unless entry

    klass, next_at = entry
    klass.set(wait_until: next_at.call(self)).perform_later(self)
  end
end
