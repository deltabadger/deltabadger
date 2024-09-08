class Admin::DashboardController < Admin::ApplicationController
  def index
    subscriptions_repository = SubscriptionsRepository.new
    legendary_plan_stats = PaymentsManager::LegendaryPlanStatsCalculator.call.data
    render :index, locals: {
      number_of_all_users: User.count,
      number_of_standard_plans: subscriptions_repository.number_of_active_subscriptions('standard'),
      number_of_pro_plans: subscriptions_repository.number_of_active_subscriptions('pro'),
      number_of_legendary_plans_sold: legendary_plan_stats[:legendary_plans_sold_count],
      number_of_legendary_plans_available: legendary_plan_stats[:legendary_plans_for_sale_count],
      number_of_all_bots: Bot.count,
      number_of_working_bots: BotsRepository.new.count_with_status('working')
    }
  end

  def model_name
    :dashboard
  end
end
