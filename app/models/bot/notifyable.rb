module Bot::Notifyable
  extend ActiveSupport::Concern

  def notify_about_error(errors: [])
    BotAlertsMailer.with(
      user: user,
      bot: self,
      errors: errors
    ).notify_about_error.deliver_later
  end

  # The bot has been stopped and will not run again until the user fixes something. Deliberately a
  # separate mail from notify_about_error, which says "something went wrong with the last
  # transaction" — accurate for a blip, badly wrong for a bot that is no longer running. It is also
  # the one bot-failure mail exempt from the daily budget: it is sent once, at the moment the bot
  # stops, and there is no later one to fall back on.
  def notify_stopped_by_error(errors: [])
    BotAlertsMailer.with(
      user: user,
      bot: self,
      errors: errors
    ).stopped_by_error.deliver_later
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

  # Sell-side mirror — denominated in BASE ("sold the whole N BTC"), not the quote-hardcoded copy.
  def notify_stopped_by_base_amount_limit
    BotAlertsMailer.with(
      user: user,
      label: label,
      amount: base_amount_limit,
      base: base_asset.symbol
    ).stopped_by_base_amount_limit.deliver_later
  end
end
