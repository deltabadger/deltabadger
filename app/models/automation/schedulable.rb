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
                   :last_action_job_at

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

    # Use raw started_at from DB, not the decorated version that may include
    # price limit condition timestamps - schedule should be based on when bot started
    checkpoint = read_attribute(:started_at) || Time.current
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

  def last_interval_checkpoint_at
    next_interval_checkpoint_at - effective_interval_duration
  end

  def progress_percentage
    if last_action_job_at.present? && next_action_job_at.present?
      (Time.current - last_action_job_at) / (next_action_job_at - last_action_job_at)
    else
      0
    end
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
                                  .find_each do |execution|
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
