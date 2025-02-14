class BotJob < ApplicationJob
  queue_as do
    bot_id = arguments.first
    bot = Bot.find(bot_id)
    bot.exchange.name.downcase.to_sym
  end
end
