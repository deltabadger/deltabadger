class Admin::DashboardController < Admin::ApplicationController
  def index
    legendary_plan = SubscriptionPlan.legendary
    render :index, locals: {
      number_of_all_users: User.count,
      number_of_mini_plans: SubscriptionPlan.mini.active_subscriptions_count + SubscriptionPlan.mini_research.active_subscriptions_count,
      number_of_standard_plans: SubscriptionPlan.standard.active_subscriptions_count + SubscriptionPlan.standard_research.active_subscriptions_count,
      number_of_pro_plans: SubscriptionPlan.pro.active_subscriptions_count,
      number_of_research_plans: SubscriptionPlan.research.active_subscriptions_count + SubscriptionPlan.mini_research.active_subscriptions_count + SubscriptionPlan.standard_research.active_subscriptions_count,
      number_of_legendary_plans_sold: legendary_plan.active_subscriptions_count,
      number_of_legendary_plans_available: legendary_plan.for_sale_count,
      number_of_all_bots: Bot.count,
      number_of_working_bots: Bot.working.count
    }
  end

  def model_name
    :dashboard
  end
end
