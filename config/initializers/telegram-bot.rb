# Configuration of Telegram bot
Telegram.bots_config = {
  default: ENV['TELEGRAM_BOT_TOKEN']
}
Rails.application.configure do
  config.telegram_updates_controller.session_store = Rails.cache
end
Telegram.bot == Telegram.bots[:default]
Telegram.bot.delete_webhook
Telegram.bot.get_updates