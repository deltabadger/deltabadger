class SubtractCredits < BaseService
  def initialize(
    subscriptions_repository: SubscriptionsRepository.new,
    convert_currency_to_credits: ConvertCurrencyToCredits.new
  )
    @subscriptions_repository = subscriptions_repository
    @convert_currency_to_credits = convert_currency_to_credits
  end

  def call(bot, const)
    first_month_and_limit_reached = bot.user.first_month? && bot.user.limit_reached?
    return nil if bot.user.unlimited? || first_month_and_limit_reached

    Rails.logger.info("Subtracting credits for user #{bot.user.id} for bot #{bot.id}")
    subscription = bot.user.subscription
    credits_to_subtract = @convert_currency_to_credits.call(
      amount: const,
      currency: bot.quote
    )

    Rails.logger.info("Subtracting credits for subscription #{subscription.id}")
    subtracted_credits = subscription.credits - credits_to_subtract

    @subscriptions_repository
      .update(subscription.id, credits: subtracted_credits)
  end
end
