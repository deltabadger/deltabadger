class SubtractCredits < BaseService
  def initialize(
    subscriptions_repository: SubscriptionsRepository.new,
    convert_currency_to_credits: ConvertCurrencyToCredits.new
  )
    @subscriptions_repository = subscriptions_repository
    @convert_currency_to_credits = convert_currency_to_credits
  end

  def call(bot, const)
    return nil if bot.user.unlimited? || bot.user.first_month?

    subscription = bot.user.subscription
    credits_to_subtract = @convert_currency_to_credits.call(
      amount: const,
      currency: bot.quote
    )

    subtracted_credits = subscription.credits - credits_to_subtract

    @subscriptions_repository
      .update(subscription.id, credits: subtracted_credits)
  end
end
