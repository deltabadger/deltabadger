class SendTelegramUpdateWorker
  include Sidekiq::Worker
  def perform(once = false, reschedule_hour = 8)
    BotsRepository.new.send_top_bots_update
    next_time = Time.new(Time.now.year, Time.now.month, Time.now.day + 1, reschedule_hour - 2)
    SendTelegramUpdateWorker.perform_at(next_time, once) unless once
  end
end
