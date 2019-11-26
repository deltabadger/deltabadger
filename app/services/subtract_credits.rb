class SubtractCredits < BaseService
  def initialize(subscriptions_repository: SubscriptionsRepository.new)
    @subscriptions_repository = subscriptions_repository
  end

  def call(bot)
    subscription = bot.user.subscription
    subtracted_credits = subscription.credits - bot.price.to_i

    @subscriptions_repository
      .update(subscription.id, credits: subtracted_credits)
  end
end
