# The bot settings page's save: each widget on the page posts a few keys of one form, and this writes
# them. BotsController#update and BotApi::Bots::UpdateSettings (MCP and REST) both save through here,
# so an automation client changes a bot exactly the way the page does.
module Bot::SettingsForm
  # `form` holds the page's field names (`quote_amount`, `limit_ordered`, `start_at`, …) with the values
  # the page posts: strings, '1'/'0' for a switch, a blank for a cleared field. False on a validation
  # failure, with the errors on the bot.
  def self.save(bot, form)
    # Before the edit, so the carry is what was owed under the settings as they were. A signal bot has none.
    bot.set_missed_quote_amount if bot.respond_to?(:set_missed_quote_amount)
    bot.update({
      settings: bot.settings.merge(bot.parse_params(form).stringify_keys),
      exchange_id: form[:exchange_id],
      label: form[:label].presence
    }.compact)
  end
end
