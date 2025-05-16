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
    return if last_end_of_funds_notification.present? && last_end_of_funds_notification > 1.day.ago

    update!(last_end_of_funds_notification: Time.current)
    BotAlertsMailer.with(
      user: user,
      bot: self,
      quote: Asset.find_by(id: quote_asset_id).symbol
    ).end_of_funds.deliver_later
  end

  def notify_stopped_by_amount_limit
    BotAlertsMailer.with(
      user: user,
      label: label,
      amount: quote_amount_limit,
      quote: quote_asset.symbol
    ).stopped_by_amount_limit.deliver_later
  end
end
