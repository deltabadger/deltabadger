class Admin::DashboardController < Admin::ApplicationController
  EARLY_BIRD_DISCOUNT_INITIAL_VALUE = (ENV.fetch('EARLY_BIRD_DISCOUNT_INITIAL_VALUE').to_i || 0).freeze

  def index
    subscriptions_repository = SubscriptionsRepository.new
    render :index, locals: {
      number_of_all_users: User.count,
      number_of_investor_plans: subscriptions_repository.number_of_active_subscriptions('investor'),
      number_of_hodler_plans: subscriptions_repository.number_of_active_subscriptions('hodler'),
      number_of_legendary_badger_plans: subscriptions_repository.number_of_active_subscriptions('legendary_badger'),
      number_of_available_legendary_badger_plans: EARLY_BIRD_DISCOUNT_INITIAL_VALUE - subscriptions_repository.number_of_active_subscriptions('legendary_badger'),
      number_of_all_bots: Bot.count,
      number_of_working_bots: BotsRepository.new.count_with_status('working')
    }
  end

  def model_name
    :dashboard
  end
end
