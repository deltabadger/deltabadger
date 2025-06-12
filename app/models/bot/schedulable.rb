module Bot::Schedulable
  extend ActiveSupport::Concern

  included do
    store_accessor :transient_data,
                   :last_action_job_at
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
    return Time.current if interval_duration.zero?

    checkpoint = started_at || Time.current
    if interval_duration == 1.month
      # handle the month interval independently so Rails can target the next same day of the month
      loop do
        checkpoint += interval_duration
        return checkpoint if checkpoint > Time.current
      end
    else
      intervals_since_checkpoint = ((Time.current - checkpoint) / interval_duration.seconds).ceil
      checkpoint + (intervals_since_checkpoint * interval_duration)
    end
  end

  def last_interval_checkpoint_at
    return legacy_last_interval_checkpoint_at if legacy?

    next_interval_checkpoint_at - interval_duration
  end

  def progress_percentage
    if last_action_job_at.present? && next_action_job_at.present?
      (Time.current - last_action_job_at) / (next_action_job_at - last_action_job_at)
    else
      0
    end
  end

  private

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
