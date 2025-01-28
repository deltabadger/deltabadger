class SendTelegramUpdateJob < ApplicationJob
  queue_as :default

  def perform
    top_bots_text = Bot.top_bots_text(update: true)
    return unless should_send_update(top_bots_text[:changed])

    Telegram.bot.send_message(chat_id: ENV['TELEGRAM_GROUP_ID'],
                              text: top_bots_text[:reply_text],
                              parse_mode: 'html')
  end

  private

  def should_send_update(changed)
    Time.now.monday? || changed
  end
end
