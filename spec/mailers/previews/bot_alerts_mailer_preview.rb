# Preview all emails at http://localhost:3000/rails/mailers/bot_alerts_mailer
class BotAlertsMailerPreview < ActionMailer::Preview
  def notify_about_error
    user = User.new(email: 'test@example.com', name: 'Mathias')
    bot = Bot.new(exchange_id: 1)
    BotAlertsMailer.with(
      user: user,
      bot: bot,
      errors: ['API error']
    ).notify_about_error
  end

  def notify_about_restart
    user = User.new(email: 'test@example.com', name: 'Mathias')
    bot = Bot.new(exchange_id: 1)
    BotAlertsMailer.with(
      user: user,
      bot: bot,
      restart_at: Time.current + 5.minutes,
      errors: ['API error']
    ).notify_about_restart
  end

  def end_of_funds
    user = User.new(email: 'test@example.com', name: 'Mathias')
    bot = Bot.new(exchange_id: 1)
    BotAlertsMailer.with(
      user: user,
      bot: bot,
      quote: 'USDT'
    ).end_of_funds
  end

end
