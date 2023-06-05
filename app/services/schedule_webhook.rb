class ScheduleWebhook < BaseService
  STARTING_BOTS_QUEUE = 'starting_bots'.freeze
  def initialize(
    bots_repository: BotsRepository.new,
    # make_webhook_worker: MakeTransactionWorker,
    make_webhook_worker: MakeWebhookWorker
    # parse_interval: ParseInterval.new,
    # next_bot_webhook_at: NextTradingBotTransactionAt.new
  )
    @bots_repository = bots_repository
    @make_webhook_worker = make_webhook_worker
    # @parse_interval = parse_interval
    # @next_bot_webhook_at = next_bot_webhook_at
  end

  def call(bot, webhook)
    # byebug
    # if bot.restarts.zero? && bot.delay.positive?
    #   bot = decrease_delay(bot)
    # elsif bot.restarts.positive?
    #   bot = increase_delay(bot)
    # end

    # next_webhook_at = next_bot_webhook_at.call(bot, first_webhook: first_webhook)
    queue_name = get_queue_name(bot)
    make_webhook_worker.sidekiq_options(queue: queue_name)
    # make_webhook_worker.new.perform(
    make_webhook_worker.perform_at(
      Time.now,
      bot.id,
      webhook
    )
  end

  private

  def get_queue_name(bot)
    bot.exchange.name.downcase
  end

  # def decrease_delay(bot)
  #   interval = parse_interval.call(bot).to_i
  #   new_delay = [bot.delay - bot.current_delay, 0].max
  #   new_current_delay = [new_delay, interval].min
  #   bots_repository.update(bot.id, delay: new_delay, current_delay: new_current_delay)
  # end
  #
  # def increase_delay(bot)
  #   last_webhook_at = bot.last_webhook.created_at
  #   next_webhook_at = next_bot_webhook_at.call(bot)
  #   new_delay = (next_webhook_at - last_webhook_at).to_i
  #   bots_repository.update(bot.id, delay: bot.delay + new_delay)
  # end

  attr_reader :bots_repository, :make_webhook_worker#, :parse_interval, :next_bot_webhook_at
end
