class TelegramWebhooksController < Telegram::Bot::UpdatesController
  include Telegram::Bot::UpdatesController::MessageContext

  def start!(*)
    reply_text = "Welcome to Deltabadger bot.\nType /help for available commands."
    respond_with :message, text: reply_text
  end

  def help!(*)
    reply_text = 'Available commands:'
    reply_text += "\n/start - Greeting"
    reply_text += "\n/help  - This list of commands."
    reply_text += "\n/top10 - List of top 10 currencies by the number of bots."
    respond_with :message, text: reply_text
  end

  def top10!(*)
    reply_text = "<b>Top 10 currencies by the number of bots:</b>\n"
    top_bots = BotsRepository.new.top_ten_bots
    return respond_with :message, text: 'No bots are working at the moment.' if top_bots.empty?

    top_bots.each_with_index { |data, index|
      reply_text += "\n#{index + 1}. #{data[:name]} - #{data[:counter]} "
      reply_text += '⬆️' if data[:is_up]
    }
    respond_with :message, text: reply_text, parse_mode: 'html'
  end
end
