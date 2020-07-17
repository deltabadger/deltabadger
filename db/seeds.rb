Exchange.find_or_create_by!(name: 'Kraken')
Exchange.find_or_create_by!(name: 'Deribit')
Exchange.find_or_create_by!(name: 'BitBay')
Exchange.find_or_create_by!(name: 'BitClude')

saver = SubscriptionPlan.find_or_create_by!(name: 'saver', cost_eu: 0, cost_other: 0, unlimited: false, years: 1, credits: 500)
investor = SubscriptionPlan.find_or_create_by!(name: 'investor', cost_eu: 20, cost_other: 20, unlimited: true, years: 1, credits: 500)
hodler = SubscriptionPlan.find_or_create_by!(name: 'hodler', cost_eu: 60, cost_other: 60, unlimited: true, years: 4, credits: 500)

User.find_or_create_by(
  email: "test@test.com"
) do |user|
  user.password = "polopolo"
  user.confirmed_at = user.confirmed_at || Time.now
end

User.find_or_create_by(
  email: "free@test.com"
) do |user|
  user.password = "polopolo"
  user.confirmed_at = user.confirmed_at || Time.now
  user.subscriptions << Subscription.new(subscription_plan: investor, end_time: Time.now - 30.days)
end

User.find_or_create_by(
  email: "investor@test.com"
) do |user|
  user.password = "polopolo"
  user.confirmed_at = user.confirmed_at || Time.now
  user.subscriptions << Subscription.new(subscription_plan: investor, end_time: Time.now + 1.year)
end
