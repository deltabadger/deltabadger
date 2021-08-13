# Configuration of Telegram bot
Telegram.bots_config = {
  default: ENV['TELEGRAM_CHAT_BOT_TOKEN']
}
config.telegram_updates_controller.session_store = Rails.cache
Telegram.bot == Telegram.bots[:default]
Telegram.bot.get_updates