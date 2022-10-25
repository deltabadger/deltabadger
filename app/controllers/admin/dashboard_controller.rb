class Admin::DashboardController < Admin::ApplicationController

  EARLY_BIRD_DISCOUNT_INITIAL_VALUE = (ENV.fetch('EARLY_BIRD_DISCOUNT_INITIAL_VALUE').to_i || 0).freeze

  def index
    render :index, locals: {
      number_of_all_users: User.count,
      number_of_investor_plans: SubscriptionsRepository.new.all_current_count('investor'),
      number_of_hodler_plans: SubscriptionsRepository.new.all_current_count('hodler'),
      number_of_legendary_badger_plans: SubscriptionsRepository.new.all_current_count('legendary_badger'),
      number_of_available_legendary_badger_plans: EARLY_BIRD_DISCOUNT_INITIAL_VALUE - SubscriptionsRepository.new.all_current_count('legendary_badger'),
      number_of_all_bots: Bot.count,
      number_of_working_bots: BotsRepository.new.count_with_status('working')
    }
  end

  def model_name
    :dashboard
  end
end
