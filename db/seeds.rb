Exchange.find_or_create_by!(name: 'Kraken')
Exchange.find_or_create_by!(name: 'Deribit')
Exchange.find_or_create_by!(name: 'BitBay')
Exchange.find_or_create_by!(name: 'BitClude')

free = SubscriptionPlan.find_or_create_by!(name: 'free')
unlimited = SubscriptionPlan.find_or_create_by!(name: 'unlimited')

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
  user.subscriptions << Subscription.new(subscription_plan: unlimited, end_time: Time.now - 30.days)
end

User.find_or_create_by(
  email: "unlimited@test.com"
) do |user|
  user.password = "polopolo"
  user.confirmed_at = user.confirmed_at || Time.now
  user.subscriptions << Subscription.new(subscription_plan: unlimited, end_time: Time.now + 1.year)
end
