Exchanges::Binance.find_or_create_by!(name: 'Binance')
Exchanges::BinanceUs.find_or_create_by!(name: 'Binance.US')
Exchanges::Zonda.find_or_create_by!(name: 'Zonda')
Exchanges::Kraken.find_or_create_by!(name: 'Kraken')
Exchanges::CoinbasePro.find_or_create_by!(name: 'Coinbase Pro')
Exchanges::Coinbase.find_or_create_by!(name: 'Coinbase')
Exchanges::Gemini.find_or_create_by!(name: 'Gemini')
Exchanges::Ftx.find_or_create_by!(name: 'FTX')
Exchanges::Bitso.find_or_create_by!(name: 'Bitso')
Exchanges::Kucoin.find_or_create_by!(name: 'KuCoin')
Exchanges::FtxUs.find_or_create_by!(name: 'FTX.US')
Exchanges::Bitfinex.find_or_create_by!(name: 'Bitfinex')
Exchanges::Bitstamp.find_or_create_by!(name: 'Bitstamp')
Exchanges::ProbitGlobal.find_or_create_by!(name: 'ProBit Global')

SettingFlag.find_or_create_by!(name: 'show_bitcoin_payment', value: true)
SettingFlag.find_or_create_by!(name: 'show_wire_payment', value: true)
SettingFlag.find_or_create_by!(name: 'show_zen_payment', value: true)

COINBASE_API_KEY = ENV.fetch('COINBASE_API_KEY').freeze
COINBASE_API_SECRET = ENV.fetch('COINBASE_API_SECRET').freeze

FeeApiKey.find_or_create_by!(exchange: Exchange.find_or_create_by!(name: 'Coinbase'))
FeeApiKey.update(key: COINBASE_API_KEY)
FeeApiKey.update(secret: COINBASE_API_SECRET)

free_plan = SubscriptionPlan.find_or_create_by!(name: 'free')
mini_plan = SubscriptionPlan.find_or_create_by!(name: 'mini')
mini_research_plan = SubscriptionPlan.find_or_create_by!(name: 'mini_research')
standard_plan = SubscriptionPlan.find_or_create_by!(name: 'standard')
standard_research_plan = SubscriptionPlan.find_or_create_by!(name: 'standard_research')
pro_plan = SubscriptionPlan.find_or_create_by!(name: 'pro')
legendary_plan = SubscriptionPlan.find_or_create_by!(name: 'legendary')
research_plan = SubscriptionPlan.find_or_create_by!(name: 'research')

free_plan_variant = SubscriptionPlanVariant.find_or_create_by!(subscription_plan: free_plan, cost_eur: 0, cost_usd: 0)

mini_plan_1_year_variant = SubscriptionPlanVariant.find_or_create_by!(subscription_plan: mini_plan, years: 1, cost_eur: 90, cost_usd: 90)
SubscriptionPlanVariant.find_or_create_by!(subscription_plan: mini_plan, years: 4, cost_eur: 270, cost_usd: 270)
SubscriptionPlanVariant.find_or_create_by!(subscription_plan: mini_plan, years: 0, cost_eur: 9, cost_usd: 9)

mini_research_plan_1_year_variant = SubscriptionPlanVariant.find_or_create_by!(subscription_plan: mini_research_plan, years: 1, cost_eur: 90 + 140, cost_usd: 90 + 140)
SubscriptionPlanVariant.find_or_create_by!(subscription_plan: mini_research_plan, years: 4, cost_eur: 270 + 420, cost_usd: 270 + 420)
SubscriptionPlanVariant.find_or_create_by!(subscription_plan: mini_research_plan, years: 0, cost_eur: 9 + 14, cost_usd: 9 + 14)

standard_plan_1_year_variant = SubscriptionPlanVariant.find_or_create_by!(subscription_plan: standard_plan, years: 1, cost_eur: 290, cost_usd: 290)
SubscriptionPlanVariant.find_or_create_by!(subscription_plan: standard_plan, years: 4, cost_eur: 870, cost_usd: 870)
SubscriptionPlanVariant.find_or_create_by!(subscription_plan: standard_plan, years: 0, cost_eur: 29, cost_usd: 29)

standard_research_plan_1_year_variant = SubscriptionPlanVariant.find_or_create_by!(subscription_plan: standard_research_plan, years: 1, cost_eur: 290 + 140, cost_usd: 290 + 140)
SubscriptionPlanVariant.find_or_create_by!(subscription_plan: standard_research_plan, years: 4, cost_eur: 870 + 420, cost_usd: 870 + 420)
SubscriptionPlanVariant.find_or_create_by!(subscription_plan: standard_research_plan, years: 0, cost_eur: 29 + 14, cost_usd: 29 + 14)

pro_plan_1_year_variant = SubscriptionPlanVariant.find_or_create_by!(subscription_plan: pro_plan, years: 1, cost_eur: 490, cost_usd: 490)
SubscriptionPlanVariant.find_or_create_by!(subscription_plan: pro_plan, years: 4, cost_eur: 1470, cost_usd: 1470)
SubscriptionPlanVariant.find_or_create_by!(subscription_plan: pro_plan, years: 0, cost_eur: 49, cost_usd: 49)

legendary_plan_variant = SubscriptionPlanVariant.find_or_create_by!(subscription_plan: legendary_plan, cost_eur: 10000, cost_usd: 10000)

research_plan_1_year_variant = SubscriptionPlanVariant.find_or_create_by!(subscription_plan: research_plan, years: 1, cost_eur: 140, cost_usd: 140)
SubscriptionPlanVariant.find_or_create_by!(subscription_plan: research_plan, years: 4, cost_eur: 420, cost_usd: 420)
SubscriptionPlanVariant.find_or_create_by!(subscription_plan: research_plan, years: 0, cost_eur: 14, cost_usd: 14)

VatRate.find_or_create_by!(country: 'Other', vat: 0)
VatRate.find_or_create_by!(country: 'Poland', vat: 0.23)
VatRate.find_or_create_by!(country: 'Estonia', vat: 0.24)

User.find_or_create_by(email: "test@test.com") do |user|
  user.name = "Satoshi"
  user.password = "Polo@polo1"
  user.confirmed_at = user.confirmed_at || Time.current
end

User.find_or_create_by(email: "admin@test.com") do |user|
  user.name = "Satoshi"
  user.password = "Polo@polo1"
  user.confirmed_at = user.confirmed_at || Time.current
  user.admin = true
end

User.find_or_create_by(email: "free@test.com") do |user|
  user.name = "Satoshi"
  user.password = "Polo@polo1"
  user.confirmed_at = user.confirmed_at || Time.current
  user.subscriptions << Subscription.new(subscription_plan_variant: free_plan_variant)
end

User.find_or_create_by(email: "mini@test.com") do |user|
  user.name = "Satoshi"
  user.password = "Polo@polo1"
  user.confirmed_at = user.confirmed_at || Time.current
  user.subscriptions << Subscription.new(subscription_plan_variant: mini_plan_1_year_variant)
end

User.find_or_create_by(email: "mini_research@test.com") do |user|
  user.name = "Satoshi"
  user.password = "Polo@polo1"
  user.confirmed_at = user.confirmed_at || Time.current
  user.subscriptions << Subscription.new(subscription_plan_variant: mini_research_plan_1_year_variant)
end

User.find_or_create_by(email: "standard@test.com") do |user|
  user.name = "Satoshi"
  user.password = "Polo@polo1"
  user.confirmed_at = user.confirmed_at || Time.current
  user.subscriptions << Subscription.new(subscription_plan_variant: standard_plan_1_year_variant)
end

User.find_or_create_by(email: "standard_research@test.com") do |user|
  user.name = "Satoshi"
  user.password = "Polo@polo1"
  user.confirmed_at = user.confirmed_at || Time.current
  user.subscriptions << Subscription.new(subscription_plan_variant: standard_research_plan_1_year_variant)
end

User.find_or_create_by(email: "pro@test.com") do |user|
  user.name = "Satoshi"
  user.password = "Polo@polo1"
  user.confirmed_at = user.confirmed_at || Time.current
  user.subscriptions << Subscription.new(subscription_plan_variant: pro_plan_1_year_variant)
end


User.find_or_create_by(email: "legendary@test.com") do |user|
  user.name = "Satoshi"
  user.password = "Polo@polo1"
  user.confirmed_at = user.confirmed_at || Time.current
  user.subscriptions << Subscription.new(subscription_plan_variant: legendary_plan_variant)
end

User.find_or_create_by(email: "research@test.com") do |user|
  user.name = "Satoshi"
  user.password = "Polo@polo1"
  user.confirmed_at = user.confirmed_at || Time.current
  user.subscriptions << Subscription.new(subscription_plan_variant: research_plan_1_year_variant)
end
