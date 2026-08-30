class Bot::StopJob < ApplicationJob
  queue_as :default

  def perform(bot, stop_message_key: nil)
    raise "Unable to stop bot #{bot.id}" unless bot.stop(stop_message_key: stop_message_key)

    # after stopping outside of the controller, we need to broadcast the streams the same way as
    # app/views/bots/stop.turbo_stream.erb
    if bot.dca_single_asset?
      bot.broadcast_replace_to(
        ["user_#{bot.user_id}", :bot_updates],
        target: 'settings',
        partial: 'bots/dca_single_assets/settings',
        locals: { bot: bot }
      )
    elsif bot.dca_multi_asset?
      # A basket bot reaches this path once it carries a quote-amount limit: hitting the cap stops
      # it from here, not from the controller, and without this branch its settings panel kept
      # rendering the running state.
      bot.broadcast_replace_to(
        ["user_#{bot.user_id}", :bot_updates],
        target: 'settings',
        partial: 'bots/dca_multi_assets/settings',
        locals: { bot: bot }
      )
    end
    bot.broadcast_replace_to(
      ["user_#{bot.user_id}", :bot_updates],
      target: 'exchange_select',
      partial: 'bots/exchange_select',
      locals: { bot: bot }
    )
  end
end
