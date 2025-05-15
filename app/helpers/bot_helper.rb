module BotHelper
  def bot_intervals
    # FIXME: reenable monthly once smart intervals are back on
    Bot::INTERVALS[...-1].map { |interval| [t("bot.#{interval}"), interval] }
  end

  def bot_type_label(bot)
    {
      'Bots::Barbell' => 'Barbell DCA',
      'Bots::Basic' => 'Basic DCA',
      'Bots::Withdrawal' => 'Withdrawal',
      'Bots::Webhook' => 'Webhook'
    }[bot.type]
  end
end
