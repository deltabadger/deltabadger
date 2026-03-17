class Bot::PriceDropLimitCheckJob < ApplicationJob
  queue_as :default

  def perform(bot)
    return unless bot.waiting?

    result = bot.get_price_drop_limit_condition_met?
    if result.failure?
      Rails.logger.warn("PriceDropLimitCheckJob for bot #{bot.id} failed: #{result.errors.to_sentence}. Retrying in 1 minute.")
      Bot::PriceDropLimitCheckJob.set(wait_until: 1.minute.from_now).perform_later(bot)
      return
    end

    if result.data
      bot.update!(status: :scheduled)
      Bot::ActionJob.perform_later(bot)
    else
      next_check_at = Time.now.utc.end_of_minute
      Bot::PriceDropLimitCheckJob.set(wait_until: next_check_at).perform_later(bot)
    end
  end
end
