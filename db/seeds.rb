Exchange.find_or_create_by!(name: 'Kraken')
Exchange.find_or_create_by!(name: 'Deribit')
Exchange.find_or_create_by!(name: 'BitBay')
Exchange.find_or_create_by!(name: 'BitClude')

_saver = SubscriptionPlan.find_or_create_by!(name: 'saver', cost_eu: 0, cost_other: 0, unlimited: false, years: 1, credits: 500)
investor = SubscriptionPlan.find_or_create_by!(name: 'investor', cost_eu: 49.99, cost_other: 49.99, unlimited: true, years: 1, credits: 500)
hodler = SubscriptionPlan.find_or_create_by!(name: 'hodler', cost_eu: 149.99, cost_other: 149.99, unlimited: true, years: 4, credits: 500)

User.find_or_create_by(
  email: "test@test.com"
) do |user|
  user.password = "polopolo"
  user.confirmed_at = user.confirmed_at || Time.current
end

User.find_or_create_by(
  email: "admin@test.com"
) do |user|
  user.password = "polopolo"
  user.confirmed_at = user.confirmed_at || Time.current
  user.admin = true
end

User.find_or_create_by(
  email: "free@test.com"
) do |user|
  user.password = "polopolo"
  user.confirmed_at = user.confirmed_at || Time.current
  user.subscriptions << Subscription.new(subscription_plan: investor, end_time: Time.current - 30.days)
end

User.find_or_create_by(
  email: "investor@test.com"
) do |user|
  user.password = "polopolo"
  user.confirmed_at = user.confirmed_at || Time.current
  user.subscriptions << Subscription.new(subscription_plan: investor, end_time: Time.current + investor.duration + 1.day)
end


User.find_or_create_by(
  email: "hodler@test.com"
) do |user|
  user.password = "polopolo"
  user.confirmed_at = user.confirmed_at || Time.current
  user.subscriptions << Subscription.new(subscription_plan: hodler, end_time: Time.current + hodler.duration + 1.day)
end
