class Admin::DashboardController < Admin::ApplicationController
  def index
    subscriptions_repository = SubscriptionsRepository.new
    legendary_badger_stats = PaymentsManager::LegendaryBadgerStatsCalculator.call.data
    render :index, locals: {
      number_of_all_users: User.count,
      number_of_investor_plans: subscriptions_repository.number_of_active_subscriptions('investor'),
      number_of_hodler_plans: subscriptions_repository.number_of_active_subscriptions('hodler'),
      number_of_legendary_badger_plans: legendary_badger_stats[:sold_legendary_badger_count],
      number_of_available_legendary_badger_plans: legendary_badger_stats[:for_sale_legendary_badger_count],
      number_of_all_bots: Bot.count,
      number_of_working_bots: BotsRepository.new.count_with_status('working')
    }
  end

  def model_name
    :dashboard
  end
end
