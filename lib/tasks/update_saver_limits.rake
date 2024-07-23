desc 'rake task to update saver users to the new plan limits'
task update_saver_limits: :environment do
  Subscription.find_each do |subscription|
    subscription.update(credits: subscription.credits + (100_000 - 1200))
    subscription.update(end_time: subscription.end_time + 2999.years) if subscription.subscription_plan.name == 'saver'
  end
  puts 'Subscriptions updated'
  SubscriptionPlan.find_each do |plan|
    plan.update(credits: 100_000)
    plan.update(years: 3000) if plan.name == 'saver'
  end
  puts 'Subscription plans updated'
end
