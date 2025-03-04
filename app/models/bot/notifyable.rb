module Bot::Notifyable
  extend ActiveSupport::Concern

  def notify_about_error(errors: [])
    BotAlertsMailer.with(
      user: user,
      bot: self,
      errors: errors
    ).notify_about_error.deliver_later
  end

  def notify_about_restart(errors: [], delay: 0.seconds)
    BotAlertsMailer.with(
      user: user,
      bot: self,
      restart_at: Time.current + delay,
      errors: errors
    ).notify_about_restart.deliver_later
  end

  def notify_end_of_funds
    now = Time.current
    return if (last_end_of_funds_notification || now) > 1.day.ago

    update!(last_end_of_funds_notification: now)
    BotAlertsMailer.with(
      user: user,
      bot: self
    ).end_of_funds.deliver_later
  end
end
