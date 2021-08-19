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
    reply_text = BotsRepository.new.top_bots_text
    respond_with :message, text: reply_text, parse_mode: 'html'
  end
end
