class BotJob < ApplicationJob
  def queue_name
    bot = arguments.first
    bot.exchange&.name&.downcase&.to_sym || :default
  end
end
