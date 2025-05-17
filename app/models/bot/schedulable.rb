module Bot::Schedulable
  extend ActiveSupport::Concern

  included do
    store_accessor :transient_data,
                   :last_action_job_at,
                   :last_successful_action_interval_checkpoint_at
  end

  def last_action_job_at
    value = super
    value.present? ? Time.zone.parse(value) : nil
  end

  def cancel_scheduled_action_jobs
    sidekiq_places = [
      Sidekiq::ScheduledSet.new,
      Sidekiq::Queue.new(exchange.name.downcase),
      Sidekiq::RetrySet.new
    ]
    sidekiq_places.each do |place|
      place.each do |job|
        # temporary fix for Bot::SetBarbellOrdersJob -> Bot::ActionJob
        if action_job_config[:class] == 'Bot::ActionJob'
          job.delete if job.queue == action_job_config[:queue] &&
                        ['Bot::ActionJob', 'Bot::SetBarbellOrdersJob'].include?(job.display_class) &&
                        job.display_args.first == action_job_config[:args].first
        elsif job.queue == action_job_config[:queue] &&
              job.display_class == action_job_config[:class] &&
              job.display_args.first == action_job_config[:args].first
          job.delete
        end

        # revert to this once Bot::SetBarbellOrdersJob doesn't exist
        # job.delete if job.queue == action_job_config[:queue] &&
        #               job.display_class == action_job_config[:class] &&
        #               job.display_args.first == action_job_config[:args].first
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
        # temporary fix for Bot::SetBarbellOrdersJob -> Bot::ActionJob
        if action_job_config[:class] == 'Bot::ActionJob'
          return job.at if job.queue == action_job_config[:queue] &&
                           ['Bot::ActionJob', 'Bot::SetBarbellOrdersJob'].include?(job.display_class) &&
                           job.display_args.first == action_job_config[:args].first
        elsif job.queue == action_job_config[:queue] &&
              job.display_class == action_job_config[:class] &&
              job.display_args.first == action_job_config[:args].first
          return job.at
        end

        # revert to this once Bot::SetBarbellOrdersJob doesn't exist
        # return job.at if job.queue == action_job_config[:queue] &&
        #                 job.display_class == action_job_config[:class] &&
        #                 job.display_args.first == action_job_config[:args].first
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
    return legacy_last_interval_checkpoint_at if legacy?

    next_interval_checkpoint_at - 1.public_send(interval)
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
      next_interval_checkpoint_at - 1.public_send(interval)
    when 'Bots::Withdrawal'
      transactions.last&.created_at || Time.current
    when 'Bots::Webhook'
      Time.current
    end
  end
end
