desc 'rake task to migrate plans and subscriptions to new plans'
task migrate_to_new_plans: :environment do
  mini_plan = SubscriptionPlan.find_or_create_by!(name: 'mini')
  mini_research_plan = SubscriptionPlan.find_or_create_by!(name: 'mini_research')
  standard_plan = SubscriptionPlan.find_or_create_by!(name: 'standard')
  standard_research_plan = SubscriptionPlan.find_or_create_by!(name: 'standard_research')
  pro_plan = SubscriptionPlan.find_or_create_by!(name: 'pro')
  research_plan = SubscriptionPlan.find_or_create_by!(name: 'research')

  SubscriptionPlanVariant.find_by(subscription_plan: mini_plan, years: 1).update!(cost_eur: 90, cost_usd: 90)
  SubscriptionPlanVariant.find_by(subscription_plan: mini_plan, years: 4).update!(cost_eur: 270, cost_usd: 270)
  SubscriptionPlanVariant.find_or_create_by!(subscription_plan: mini_plan, years: 0, cost_eur: 9, cost_usd: 9)
  SubscriptionPlanVariant.find_or_create_by!(subscription_plan: mini_research_plan, years: 1, cost_eur: 90 + 140, cost_usd: 90 + 140) # rubocop:disable Layout/LineLength
  SubscriptionPlanVariant.find_or_create_by!(subscription_plan: mini_research_plan, years: 4, cost_eur: 270 + 420, cost_usd: 270 + 420) # rubocop:disable Layout/LineLength
  SubscriptionPlanVariant.find_or_create_by!(subscription_plan: mini_research_plan, years: 0, cost_eur: 9 + 14, cost_usd: 9 + 14)
  SubscriptionPlanVariant.find_or_create_by!(subscription_plan: standard_plan, years: 1, cost_eur: 290, cost_usd: 290)
  SubscriptionPlanVariant.find_or_create_by!(subscription_plan: standard_plan, years: 4, cost_eur: 870, cost_usd: 870)
  SubscriptionPlanVariant.find_or_create_by!(subscription_plan: standard_plan, years: 0, cost_eur: 29, cost_usd: 29)
  SubscriptionPlanVariant.find_or_create_by!(subscription_plan: standard_research_plan, years: 1, cost_eur: 290 + 140, cost_usd: 290 + 140) # rubocop:disable Layout/LineLength
  SubscriptionPlanVariant.find_or_create_by!(subscription_plan: standard_research_plan, years: 4, cost_eur: 870 + 420, cost_usd: 870 + 420) # rubocop:disable Layout/LineLength
  SubscriptionPlanVariant.find_or_create_by!(subscription_plan: standard_research_plan, years: 0, cost_eur: 29 + 14, cost_usd: 29 + 14) # rubocop:disable Layout/LineLength
  SubscriptionPlanVariant.find_or_create_by!(subscription_plan: research_plan, years: 1, cost_eur: 140, cost_usd: 140)
  SubscriptionPlanVariant.find_or_create_by!(subscription_plan: research_plan, years: 4, cost_eur: 420, cost_usd: 420)
  SubscriptionPlanVariant.find_or_create_by!(subscription_plan: research_plan, years: 0, cost_eur: 14, cost_usd: 14)
  SubscriptionPlanVariant.find_by(subscription_plan: pro_plan, years: 1).update!(cost_eur: 490, cost_usd: 490)
  SubscriptionPlanVariant.find_by(subscription_plan: pro_plan, years: 4).update!(cost_eur: 1470, cost_usd: 1470)
  SubscriptionPlanVariant.find_or_create_by!(subscription_plan: pro_plan, years: 0, cost_eur: 49, cost_usd: 49)
end
