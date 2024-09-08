Exchange.find_or_create_by!(name: 'Binance')
Exchange.find_or_create_by!(name: 'Binance.US')
Exchange.find_or_create_by!(name: 'Zonda')
Exchange.find_or_create_by!(name: 'Kraken')
Exchange.find_or_create_by!(name: 'Coinbase Pro')
Exchange.find_or_create_by!(name: 'Coinbase')
Exchange.find_or_create_by!(name: 'Gemini')
Exchange.find_or_create_by!(name: 'FTX')
Exchange.find_or_create_by!(name: 'Bitso')
Exchange.find_or_create_by!(name: 'KuCoin')
Exchange.find_or_create_by!(name: 'FTX.US')
Exchange.find_or_create_by!(name: 'Bitfinex')
Exchange.find_or_create_by!(name: 'Bitstamp')
Exchange.find_or_create_by!(name: 'ProBit Global')

SettingFlag.find_or_create_by!(name: 'show_bitcoin_payment', value: true)
SettingFlag.find_or_create_by!(name: 'show_wire_payment', value: true)
SettingFlag.find_or_create_by!(name: 'show_stripe_payment', value: true)
SettingFlag.find_or_create_by!(name: 'show_zen_payment', value: true)

COINBASE_API_KEY = ENV.fetch('COINBASE_API_KEY').freeze
COINBASE_API_SECRET = ENV.fetch('COINBASE_API_SECRET').freeze

FeeApiKey.find_or_create_by!(exchange: Exchange.find_or_create_by!(name: 'Coinbase'))
FeeApiKey.update(key: COINBASE_API_KEY)
FeeApiKey.update(secret: COINBASE_API_SECRET)

SubscriptionPlan.find_or_create_by!(name: 'free', cost_eu: 0, cost_other: 0, unlimited: false, years: 1, credits: 1200)
standard_plan = SubscriptionPlan.find_or_create_by!(name: 'standard', cost_eu: 49.99, cost_other: 49.99, unlimited: true, years: 1, credits: 1200)
pro_plan = SubscriptionPlan.find_or_create_by!(name: 'pro', cost_eu: 149.99, cost_other: 149.99, unlimited: true, years: 4, credits: 1200)
legendary_plan = SubscriptionPlan.find_or_create_by!(name: 'legendary', cost_eu: 249.99, cost_other: 249.99, unlimited: true, years: 10000, credits: 1200)

VatRate.find_or_create_by!(country: 'Other', vat: 0)
VatRate.find_or_create_by!(country: 'Poland', vat: 0.23)
VatRate.find_or_create_by!(country: 'Estonia', vat: 0.2)

User.find_or_create_by(
  email: "test@test.com"
) do |user|
  user.name = "Jan"
  user.password = "Polo@polo1"
  user.confirmed_at = user.confirmed_at || Time.current
end

User.find_or_create_by(
  email: "admin@test.com"
) do |user|
  user.name = "Jan"
  user.password = "Polo@polo1"
  user.confirmed_at = user.confirmed_at || Time.current
  user.admin = true
end

User.find_or_create_by(
  email: "free@test.com"
) do |user|
  user.name = "Jan"
  user.password = "Polo@polo1"
  user.confirmed_at = user.confirmed_at || Time.current
  user.subscriptions << Subscription.new(subscription_plan: standard_plan, end_time: Time.current - 30.days, credits: standard_plan.credits)
end

User.find_or_create_by(
  email: "standard@test.com"
) do |user|
  user.name = "Jan"
  user.password = "Polo@polo1"
  user.confirmed_at = user.confirmed_at || Time.current
  user.subscriptions << Subscription.new(subscription_plan: standard_plan, end_time: Time.current + standard_plan.duration + 1.day, credits: standard_plan.credits)
end


User.find_or_create_by(
  email: "pro@test.com"
) do |user|
  user.name = "Jan"
  user.password = "Polo@polo1"
  user.confirmed_at = user.confirmed_at || Time.current
  user.subscriptions << Subscription.new(subscription_plan: pro_plan, end_time: Time.current + pro_plan.duration + 1.day, credits: pro_plan.credits)
end


User.find_or_create_by(
  email: "legendary@test.com"
) do |user|
  user.name = "Jan"
  user.password = "Polo@polo1"
  user.confirmed_at = user.confirmed_at || Time.current
  user.subscriptions << Subscription.new(subscription_plan: legendary_plan, end_time: Time.current + legendary_plan.duration + 1.day, credits: legendary_plan.credits)
end
