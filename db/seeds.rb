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
SettingFlag.find_or_create_by!(name: 'show_zen_payment', value: true)

COINBASE_API_KEY = ENV.fetch('COINBASE_API_KEY').freeze
COINBASE_API_SECRET = ENV.fetch('COINBASE_API_SECRET').freeze

FeeApiKey.find_or_create_by!(exchange: Exchange.find_or_create_by!(name: 'Coinbase'))
FeeApiKey.update(key: COINBASE_API_KEY)
FeeApiKey.update(secret: COINBASE_API_SECRET)

free_plan = SubscriptionPlan.find_or_create_by!(name: 'free', unlimited: false)
basic_plan = SubscriptionPlan.find_or_create_by!(name: 'basic', unlimited: true)
pro_plan = SubscriptionPlan.find_or_create_by!(name: 'pro', unlimited: true)
legendary_plan = SubscriptionPlan.find_or_create_by!(name: 'legendary', unlimited: true)

free_plan_variant = SubscriptionPlanVariant.find_or_create_by!(subscription_plan: free_plan, cost_eur: 0, cost_usd: 0)
basic_plan_1_year_variant = SubscriptionPlanVariant.find_or_create_by!(subscription_plan: basic_plan, years: 1, cost_eur: 87, cost_usd: 97)
SubscriptionPlanVariant.find_or_create_by!(subscription_plan: basic_plan, years: 4, cost_eur: 267, cost_usd: 297)
pro_plan_1_year_variant = SubscriptionPlanVariant.find_or_create_by!(subscription_plan: pro_plan, years: 1, cost_eur: 267, cost_usd: 297)
SubscriptionPlanVariant.find_or_create_by!(subscription_plan: pro_plan, years: 4, cost_eur: 797, cost_usd: 897)
legendary_plan_variant = SubscriptionPlanVariant.find_or_create_by!(subscription_plan: legendary_plan, cost_eur: 9000, cost_usd: 10000)

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
  user.subscriptions << Subscription.new(subscription_plan_variant: free_plan_variant)
end

User.find_or_create_by(
  email: "basic@test.com"
) do |user|
  user.name = "Jan"
  user.password = "Polo@polo1"
  user.confirmed_at = user.confirmed_at || Time.current
  user.subscriptions << Subscription.new(subscription_plan_variant: basic_plan_1_year_variant)
end


User.find_or_create_by(
  email: "pro@test.com"
) do |user|
  user.name = "Jan"
  user.password = "Polo@polo1"
  user.confirmed_at = user.confirmed_at || Time.current
  user.subscriptions << Subscription.new(subscription_plan_variant: pro_plan_1_year_variant)
end


User.find_or_create_by(
  email: "legendary@test.com"
) do |user|
  user.name = "Jan"
  user.password = "Polo@polo1"
  user.confirmed_at = user.confirmed_at || Time.current
  user.subscriptions << Subscription.new(subscription_plan_variant: legendary_plan_variant)
end
