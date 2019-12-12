class SubtractCredits < BaseService
  def initialize(
    subscriptions_repository: SubscriptionsRepository.new,
    convert_currency_to_credits: ConvertCurrencyToCredits.new
  )
    @subscriptions_repository = subscriptions_repository
    @convert_currency_to_credits = convert_currency_to_credits
  end

  def call(bot)
    return nil if bot.user.unlimited?

    subscription = bot.user.subscription
    credits_to_subtract = @convert_currency_to_credits.call(
      amount: bot.price.to_i,
      currency: bot.currency
    )

    subtracted_credits = subscription.credits - credits_to_subtract

    @subscriptions_repository
      .update(subscription.id, credits: subtracted_credits)
  end
end
