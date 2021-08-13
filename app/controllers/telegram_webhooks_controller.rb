class TelegramWebhooksController < Telegram::Bot::UpdatesController
  include Telegram::Bot::UpdatesController::MessageContext

  def start!(*)
    respond_with :message, text: "Welcome to Deltabadger bot.\nType /help for available commands."
  end

  def hello!(*)
    respond_with :message, text: "Welcome to Deltabadger bot.\nType /help for available commands."
  end

  def help!(*)
    respond_with :message, text: "Available commands: \n/top10"
  end

  def top10!(*)
    reply_text = 'Top 10 currencies by the number of bots:'
    top_bots = BotsRepository.new.top_ten_bots
    return respond_with :message, text: 'No bots are working at the moment.' if top_bots.empty?

    top_bots.each_with_index { |data, index|
      reply_text += "\n#{index+1}. #{data.name} - #{data.count}"
    }
    respond_with :message, text: reply_text
  end
end
