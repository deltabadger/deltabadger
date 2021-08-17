# Configuration of Telegram bot
Telegram.bots_config = {
  default: {
    token: ENV['TELEGRAM_BOT_TOKEN'],
    username: ENV['TELEGRAM_BOT_NICKNAME']
  }
}
Rails.application.configure do
  config.telegram_updates_controller.session_store = Rails.cache
end
Telegram.bot == Telegram.bots[:default]
