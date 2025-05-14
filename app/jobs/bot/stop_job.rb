class Bot::StopJob < ApplicationJob
  queue_as :default

  def perform(bot, stop_message_key: nil)
    raise "Unable to stop bot #{bot.id}" unless bot.stop(stop_message_key: stop_message_key)

    # after stopping outside of the controller, we need to broadcast the streams the same way as
    # app/views/bots/stop.turbo_stream.erb
    broadcast_replace_to(
      ["user_#{bot.user_id}", :bot_updates],
      target: 'settings',
      partial: 'bots/barbell/settings',
      locals: { bot: bot }
    )
    broadcast_replace_to(
      ["user_#{bot.user_id}", :bot_updates],
      target: 'exchange_select',
      partial: 'bots/exchange_select',
      locals: { bot: bot }
    )
  end
end
