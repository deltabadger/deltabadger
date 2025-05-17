class Bot::SetBarbellOrdersJob < BotJob
  # temporary fix for Bot::SetBarbellOrdersJob -> Bot::ActionJob
  # delete this file once Bot::SetBarbellOrdersJob doesn't exist
  def perform(bot)
    Bot::ActionJob.perform_later(bot)
  end
end
