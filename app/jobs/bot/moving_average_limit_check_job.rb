class Bot::MovingAverageLimitCheckJob < ApplicationJob
  queue_as :default

  def perform(bot)
    return unless bot.waiting?

    result = bot.get_moving_average_limit_condition_met?
    if result.failure?
      raise "Failed to check moving average limit condition for bot #{bot.id}. " \
            "Errors: #{result.errors.to_sentence}"
    end

    if result.data
      bot.update!(started_at: Time.current)
      case bot.moving_average_limit_timing_condition
      when 'after' then Bot::ActionJob.perform_later(bot)
      when 'while' then Bot::ActionJob.set(wait_until: bot.next_interval_checkpoint_at).perform_later(bot)
      end
    else
      next_check_at = Time.now.utc + Utilities::Time.seconds_to_current_candle_close(bot.moving_average_limit_in_timeframe_duration)
      Bot::MovingAverageLimitCheckJob.set(wait_until: next_check_at).perform_later(bot)
    end
  end
end
