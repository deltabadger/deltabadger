class BotJob < ApplicationJob
  limits_concurrency to: 1, key: ->(bot, *) { "exchange_#{bot.exchange&.name_id}" }

  def queue_name
    bot = arguments.first
    bot.exchange&.name_id&.to_sym || :default
  end
end
