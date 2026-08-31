module Automation::Schedulable
  extend ActiveSupport::Concern

  INTERVALS = {
    'hour' => 1.hour,
    'day' => 1.day,
    'week' => 1.week,
    'month' => 1.month
  }.freeze

  included do
    store_accessor :transient_data,
                   :last_action_job_at,
                   :waiting_for_market_open

    validates :interval, presence: true, inclusion: { in: INTERVALS.keys }
  end

  def interval_duration
    INTERVALS[interval]
  end

  def effective_interval_duration
    interval_duration
  end

  def last_action_job_at
    value = super
    value.present? ? Time.zone.parse(value) : nil
  end

  def cancel_scheduled_action_jobs
    cancel_solid_queue_jobs(
      job_class: action_job_config[:class],
      record: self
    )
  end

  def next_action_job_at
    return nil unless exchange.present?

    find_next_scheduled_job_at(
      job_class: action_job_config[:class],
      record: self
    )
  end

  # Does a live ActionJob exist for this record in ANY active Solid Queue state?
  #
  # Broader than `next_action_job_at`, which only ever sees a SCHEDULED execution: a job that is due
  # now (ready), one a worker has already claimed, and one blocked on the per-exchange concurrency
  # semaphore all report nil there while being very much alive. Bot::RepairOrphanedBotsJob asks this
  # instead, so it stops "repairing" bots that are mid-tick and enqueueing a second job for the tick.
  def pending_action_job?
    active_job?(job_class: action_job_config[:class], record: self)
  end

  def next_interval_checkpoint_at
    return Time.current if effective_interval_duration.zero?

    checkpoint = repeat_anchor_at || Time.current

    # If the anchor itself is in the future, return it as-is. Without this guard
    # the month-interval loop below would skip the first intended execution
    # (returning anchor + 1.month), and the non-month formula could produce a
    # checkpoint that lies before the anchor.
    return checkpoint if checkpoint.future?

    if effective_interval_duration == 1.month
      # handle the month interval independently so Rails can target the next same day of the month
      loop do
        checkpoint += effective_interval_duration
        return checkpoint if checkpoint > Time.current
      end
    else
      # Both sides of this must speak the same clock. The count is elapsed SECONDS, so the step
      # has to be added as seconds too (.to_f), not as a calendar duration: `time + n.weeks`
      # preserves the LOCAL WALL CLOCK, so under a DST zone it lands an hour off the grid the
      # count was measured against — before it, going into summer. A checkpoint in the past is
      # re-enqueued immediately and spins the job at machine speed until real time catches up.
      # Jobs are pinned to the app zone (ApplicationJob), so this is belt and braces — but the
      # grid must not depend on which zone the caller happens to be in either way.
      intervals_since_checkpoint = ((Time.current - checkpoint) / effective_interval_duration.seconds).ceil
      checkpoint + (intervals_since_checkpoint * effective_interval_duration.to_f)
    end
  end

  # Default — overridden by Bot::Startable. The baseline for interval math is
  # the raw started_at from DB (not a decorated version that may include price
  # limit condition timestamps).
  def repeat_anchor_at
    read_attribute(:started_at)
  end

  # Default — overridden by Bot::Startable. Schedulable models that don't opt
  # into the starting-time feature behave as the feature were disabled.
  def start_time_enabled?
    false
  end

  def last_interval_checkpoint_at
    # Step back the same way the forward grid steps forward, or the pair straddles a DST
    # transition: subtracting a calendar duration from an absolute grid point lands 25 hours back
    # in autumn, and Bot::Accountable#pending_quote_amount floors an interval count against this
    # value — one whole contribution dropped. Months keep calendar arithmetic; they are not a
    # fixed number of seconds, and the forward branch treats them the same way.
    return next_interval_checkpoint_at - effective_interval_duration if effective_interval_duration == 1.month

    next_interval_checkpoint_at - effective_interval_duration.to_f
  end

  def progress_percentage
    start_time = last_action_job_at || last_interval_checkpoint_at
    end_time = next_action_job_at

    if start_time.present? && end_time.present? && end_time > start_time
      (Time.current - start_time) / (end_time - start_time)
    else
      0
    end
  end

  def progress_start_time
    last_action_job_at || last_interval_checkpoint_at
  end

  private

  def cancel_solid_queue_jobs(job_class:, record:)
    return unless defined?(SolidQueue)

    global_id = record.to_global_id.to_s

    # Cancel scheduled jobs
    SolidQueue::ScheduledExecution.joins(:job)
                                  .where(solid_queue_jobs: { class_name: job_class.to_s })
                                  .find_each do |execution|
      execution.job.destroy if job_matches_record?(execution.job, global_id)
    end

    # Cancel ready (queued) jobs
    SolidQueue::ReadyExecution.joins(:job)
                              .where(solid_queue_jobs: { class_name: job_class.to_s })
                              .find_each do |execution|
      execution.job.destroy if job_matches_record?(execution.job, global_id)
    end

    # Cancel blocked jobs. Bot jobs are per-exchange concurrency-limited (one thread per exchange
    # queue), so an old job can sit in BlockedExecution waiting for a slot. Without this it survives a
    # stop/reschedule/flip and later unblocks to run against the (possibly reversed) bot; the semaphore
    # self-heals via its concurrency-duration expiry.
    SolidQueue::BlockedExecution.joins(:job)
                                .where(solid_queue_jobs: { class_name: job_class.to_s })
                                .find_each do |execution|
      execution.job.destroy if job_matches_record?(execution.job, global_id)
    end
  end

  # Collapse duplicate live jobs of one class for one record down to a single survivor, keeping the
  # NEWEST and destroying the rest.
  #
  # "Keep one" rather than "delete all but the one I just enqueued" is what makes this safe under
  # concurrency. Two chains running at once would both enqueue a successor and then both prune; an
  # exclusion list has each prune spare its OWN successor and destroy the other's, which can end
  # with zero jobs and a bot that never polls again. Keeping whichever is newest is idempotent and
  # order-independent: every interleaving converges on exactly one, and it can never reach zero.
  #
  # Claimed executions are deliberately not candidates — that is the job currently running.
  def prune_duplicate_solid_queue_jobs(job_class:, record:)
    return unless defined?(SolidQueue)

    global_id = record.to_global_id.to_s
    jobs = [SolidQueue::ScheduledExecution, SolidQueue::ReadyExecution,
            SolidQueue::BlockedExecution].flat_map do |execution_model|
      live_executions_for(execution_model, job_class, global_id).map(&:job)
    end

    return if jobs.size <= 1

    # Re-check for a claimed execution immediately before destroying. A ready job can be picked up
    # by a worker between the select above and here, and destroying it would cascade to the claimed
    # execution and abort a running poll. Skipping it is free: it is a duplicate, so the survivor
    # already keeps the chain alive, and the next tick collapses it once it is no longer claimed.
    # This narrows the race rather than closing it — the alternative, locking the queue tables on
    # every poll, costs more than the mid-flight abort of a redundant poll it would prevent.
    jobs.sort_by(&:id)[0..-2].each do |job|
      next if SolidQueue::ClaimedExecution.exists?(job_id: job.id)

      job.destroy
    end
  end

  def find_next_scheduled_job_at(job_class:, record:)
    return nil unless defined?(SolidQueue)

    global_id = record.to_global_id.to_s

    SolidQueue::ScheduledExecution.joins(:job)
                                  .where(solid_queue_jobs: { class_name: job_class.to_s })
                                  .order(:scheduled_at)
                                  .each do |execution|
      return execution.scheduled_at.in_time_zone if job_matches_record?(execution.job, global_id)
    end

    nil
  end

  # True if a job of job_class for `record` exists in ANY active Solid Queue execution state
  # (Scheduled / Ready / Claimed / Blocked). Excludes FailedExecution by design — a dead-lettered
  # job is NOT a live chain. Used by limit-check and orphan recovery to avoid double-enqueuing.
  def active_job?(job_class:, record:)
    return false unless defined?(SolidQueue)

    global_id = record.to_global_id.to_s
    [SolidQueue::ScheduledExecution, SolidQueue::ReadyExecution,
     SolidQueue::ClaimedExecution, SolidQueue::BlockedExecution].any? do |execution_model|
      execution_model.joins(:job)
                     .where(solid_queue_jobs: { class_name: job_class.to_s })
                     .any? { |execution| job_matches_record?(execution.job, global_id) }
    end
  end

  # Executions of job_class belonging to record, narrowed in SQL before the exact Ruby check.
  #
  # The LIKE is a pure prefilter: the serialized arguments embed the GlobalID verbatim (either as
  # "_aj_globalid" or as a bare string), so it can only exclude rows job_matches_record? would
  # reject anyway. Without it every caller loads EVERY live job of that class and filters in Ruby
  # — fine for a one-off cancel, but the limit-check prune runs once per waiting bot per minute,
  # which made it scale with the square of the waiting-bot count.
  #
  # ponytail: the leading wildcard means this cannot use an index, so it still SCANS every
  # class-matching row — it only avoids instantiating and JSON-parsing them. The cost is bounded by
  # the number of waiting limit bots in ONE installation, which is small in practice: a few dozen
  # bots costs a few hundred row-scans a minute. If an installation ever runs a few hundred waiting
  # limit bots, index it properly — a generated column over the GlobalID, or give the limit-check
  # jobs a per-bot concurrency_key, which is already indexed.
  def live_executions_for(execution_model, job_class, global_id)
    execution_model.joins(:job)
                   .where(solid_queue_jobs: { class_name: job_class.to_s })
                   .where('solid_queue_jobs.arguments LIKE ?', "%#{global_id}%")
                   .includes(:job)
                   .select { |execution| job_matches_record?(execution.job, global_id) }
  end

  def job_matches_record?(job, global_id)
    return false unless job&.arguments.present?

    # SolidQueue stores arguments as a Hash with "arguments" key containing the actual job args
    args = job.arguments.is_a?(Hash) ? job.arguments['arguments'] : job.arguments
    return false unless args.is_a?(Array)

    # ActiveJob serializes GlobalID records as { "_aj_globalid" => "..." }
    args.any? do |arg|
      (arg.is_a?(Hash) && arg['_aj_globalid'] == global_id) ||
        (arg.is_a?(String) && arg == global_id)
    end
  end
end
