Exchange.find_or_create_by!(name: 'Binance')
Exchange.find_or_create_by!(name: 'Binance.US')
Exchange.find_or_create_by!(name: 'Zonda')
Exchange.find_or_create_by!(name: 'Kraken')
Exchange.find_or_create_by!(name: 'Coinbase Pro')
Exchange.find_or_create_by!(name: 'Gemini')
Exchange.find_or_create_by!(name: 'FTX')
Exchange.find_or_create_by!(name: 'Bitso')
Exchange.find_or_create_by!(name: 'KuCoin')
Exchange.find_or_create_by!(name: 'FTX.US')
Exchange.find_or_create_by!(name: 'Bitfinex')
Exchange.find_or_create_by!(name: 'Bitstamp')
Exchange.find_or_create_by!(name: 'ProBit Global')

_saver = SubscriptionPlan.find_or_create_by!(name: 'saver', cost_eu: 0, cost_other: 0, unlimited: false, years: 1, credits: 1200)
investor = SubscriptionPlan.find_or_create_by!(name: 'investor', cost_eu: 49.99, cost_other: 49.99, unlimited: true, years: 1, credits: 1200)
hodler = SubscriptionPlan.find_or_create_by!(name: 'hodler', cost_eu: 149.99, cost_other: 149.99, unlimited: true, years: 4, credits: 1200)

VatRate.find_or_create_by!(country: 'Other', vat: 0)
VatRate.find_or_create_by!(country: 'Poland', vat: 0.23)
VatRate.find_or_create_by!(country: 'Estonia', vat: 0.2)

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
  user.subscriptions << Subscription.new(subscription_plan: investor, end_time: Time.current - 30.days, credits: investor.credits)
end

User.find_or_create_by(
  email: "investor@test.com"
) do |user|
  user.password = "polopolo"
  user.confirmed_at = user.confirmed_at || Time.current
  user.subscriptions << Subscription.new(subscription_plan: investor, end_time: Time.current + investor.duration + 1.day, credits: investor.credits)
end


User.find_or_create_by(
  email: "hodler@test.com"
) do |user|
  user.password = "polopolo"
  user.confirmed_at = user.confirmed_at || Time.current
  user.subscriptions << Subscription.new(subscription_plan: hodler, end_time: Time.current + hodler.duration + 1.day, credits: hodler.credits)
end
