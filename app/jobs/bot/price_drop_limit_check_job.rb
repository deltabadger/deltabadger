class Bot::PriceDropLimitCheckJob < ApplicationJob
  queue_as :default

  def perform(bot)
    return unless bot.waiting?

    result = bot.get_price_drop_limit_condition_met?
    if result.failure?
      raise "Failed to check price drop limit condition for bot #{bot.id}. " \
            "Errors: #{result.errors.to_sentence}"
    end

    if result.data
      bot.update!(started_at: Time.current)
      Bot::ActionJob.perform_later(bot)
    else
      next_check_at = Time.now.utc.end_of_minute
      Bot::PriceDropLimitCheckJob.set(wait_until: next_check_at).perform_later(bot)
    end
  end
end
