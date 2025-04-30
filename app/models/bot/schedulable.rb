module Bot::Schedulable
  extend ActiveSupport::Concern

  included do
    store_accessor :transient_data, :last_action_job_at_iso8601
  end

  def next_action_job_at
    return nil unless exchange.present?

    sidekiq_places = [
      Sidekiq::RetrySet.new,
      Sidekiq::ScheduledSet.new
    ]
    sidekiq_places.each do |place|
      place.each do |job|
        return job.at if job.queue == action_job_config[:queue] &&
                         job.display_class == action_job_config[:class] &&
                         job.display_args.first == action_job_config[:args].first
      end
    end
    nil
  end

  def next_interval_checkpoint_at
    return legacy_next_interval_checkpoint_at if legacy?

    checkpoint = started_at || Time.current
    loop do
      checkpoint += 1.public_send(interval)
      return checkpoint if checkpoint > Time.current
    end
  end

  def last_interval_checkpoint_at
    next_interval_checkpoint_at - 1.public_send(interval)
  end

  def progress_percentage
    if last_action_job_at_iso8601.present? && next_action_job_at.present?
      last_action_job_at = DateTime.parse(last_action_job_at_iso8601)
      (Time.current - last_action_job_at) / (next_action_job_at - last_action_job_at)
    else
      0
    end
  end

  private

  def legacy_next_interval_checkpoint_at
    NextTradingBotTransactionAt.new.call(self) || Time.current
  end
end
