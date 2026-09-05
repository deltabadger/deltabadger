class Bot::BroadcastAfterScheduledActionJob < ApplicationJob
  queue_as :default

  def perform(bot)
    # This loop makes sure Solid Queue has time to schedule the job. A signal bot has no next tick
    # to wait for — it would spin the whole five seconds on every app wake.
    if bot.respond_to?(:pending_action_job?)
      50.times do
        break if bot.next_action_job_at.present?

        sleep 0.1
      end
    end

    bot.broadcast_status_bar_update
  end
end
