class SendTelegramUpdateWorker
  include Sidekiq::Worker
  def perform(reschedule_hour = 8)
    BotsRepository.new.send_top_bots_update
    next_day = (Time.now + 1.day)
    reschedule_hour -= (Time.now.in_time_zone('Europe/Warsaw').utc_offset / 1.hour)
    next_time = Time.new(next_day.year,
                         next_day.month,
                         next_day.day,
                         reschedule_hour)
    SendTelegramUpdateWorker.perform_at(next_time)
  end
end
