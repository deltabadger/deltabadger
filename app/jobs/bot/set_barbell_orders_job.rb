class SetBarbellOrdersJob < BotJob
  def perform(bot_id)
    bot = Bot.find(bot_id)
    bot.set_barbell_orders
    # verify what fails etc
  end
end
