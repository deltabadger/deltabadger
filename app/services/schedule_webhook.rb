class ScheduleWebhook < BaseService
  STARTING_BOTS_QUEUE = 'starting_bots'.freeze
  def initialize(
    make_webhook_worker: MakeWebhookWorker
  )
    @make_webhook_worker = make_webhook_worker
  end

  def call(bot, webhook)
    queue_name = get_queue_name(bot)
    @make_webhook_worker.sidekiq_options(queue: queue_name)
    @make_webhook_worker.perform_at(
      Time.now,
      bot.id,
      webhook
    )
  end

  private

  def get_queue_name(bot)
    bot.exchange.name.downcase
  end
end
