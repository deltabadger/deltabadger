class Bot::PriceLimitCheckJob < ApplicationJob
  queue_as :default

  def perform(bot)
    return unless bot.waiting?

    result = bot.get_price_limit_condition_met?
    raise "Failed to check price limit condition for bot #{bot.id}. Errors: #{result.errors.to_sentence}" unless result.success?

    if result.data
      bot.update!(started_at: Time.current)
      Bot::ActionJob.perform_later(bot)
    else
      next_check_at = Time.now.utc.end_of_minute
      Bot::PriceLimitCheckJob.set(wait_until: next_check_at).perform_later(bot)
    end
  end
end
