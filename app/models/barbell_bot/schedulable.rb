module BarbellBot::Schedulable
  extend ActiveSupport::Concern

  included do
    store_accessor :transient_data, :last_set_barbell_orders_job_at_iso8601
  end

  def next_set_barbell_orders_job_at
    return nil unless exchange.present?

    sidekiq_places = [
      Sidekiq::RetrySet.new,
      Sidekiq::ScheduledSet.new
    ]
    sidekiq_places.each do |place|
      place.each do |job|
        return job.at if job.queue == exchange.name.downcase &&
                         job.display_class == 'Bot::SetBarbellOrdersJob' &&
                         job.display_args == [{ '_aj_globalid' => to_global_id.to_s }]
      end
    end
    nil
  end

  def next_interval_checkpoint_at
    checkpoint = started_at
    loop do
      checkpoint += 1.public_send(interval)
      return checkpoint if checkpoint > Time.current
    end
  end

  def progress_percentage
    if last_set_barbell_orders_job_at_iso8601.present? && next_set_barbell_orders_job_at.present?
      last_set_barbell_orders_job_at = DateTime.parse(last_set_barbell_orders_job_at_iso8601)
      (Time.current - last_set_barbell_orders_job_at) / (next_set_barbell_orders_job_at - last_set_barbell_orders_job_at)
    else
      0
    end
  end
end
