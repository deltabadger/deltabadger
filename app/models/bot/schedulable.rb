module Bot::Schedulable
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

    validates :interval, presence: true, inclusion: { in: INTERVALS.keys }, unless: :legacy?
    validate :validate_interval_included_in_subscription_plan, on: :start, unless: :legacy?
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
    sidekiq_places = [
      Sidekiq::ScheduledSet.new,
      Sidekiq::Queue.new(exchange.name_id),
      Sidekiq::RetrySet.new
    ]
    sidekiq_places.each do |place|
      place.each do |job|
        job.delete if job.queue == action_job_config[:queue] &&
                      job.display_class == action_job_config[:class] &&
                      job.display_args.first == action_job_config[:args].first
      end
    end
  end

  def next_action_job_at
    return nil unless exchange.present?

    sidekiq_places = [
      Sidekiq::RetrySet.new,
      Sidekiq::ScheduledSet.new
    ]
    sidekiq_places.each do |place|
      place.each do |job|
        return job.at.in_time_zone if job.queue == action_job_config[:queue] &&
                                      job.display_class == action_job_config[:class] &&
                                      job.display_args.first == action_job_config[:args].first
      end
    end
    nil
  end

  def next_interval_checkpoint_at
    return legacy_next_interval_checkpoint_at if legacy?
    return Time.current if effective_interval_duration.zero?

    checkpoint = started_at || Time.current
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
    return legacy_last_interval_checkpoint_at if legacy?

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

  def validate_interval_included_in_subscription_plan
    return if user.subscription.paid?

    errors.add(:user, :upgrade) if interval != 'day'
  end

  def legacy_next_interval_checkpoint_at
    NextTradingBotTransactionAt.new.call(self) || Time.current
  end

  def legacy_last_interval_checkpoint_at
    case type
    when 'Bots::Basic'
      next_interval_checkpoint_at - interval_duration
    when 'Bots::Withdrawal'
      transactions.last&.created_at || Time.current
    when 'Bots::Webhook'
      Time.current
    end
  end
end
