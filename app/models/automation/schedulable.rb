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
      intervals_since_checkpoint = ((Time.current - checkpoint) / effective_interval_duration.seconds).ceil
      checkpoint + (intervals_since_checkpoint * effective_interval_duration)
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
    next_interval_checkpoint_at - effective_interval_duration
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
