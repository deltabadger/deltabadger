class BotJob < ApplicationJob
  def queue_name
    bot = arguments.first
    bot.exchange&.name_id&.to_sym || :default
  end
end
