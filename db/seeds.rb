binance = Exchanges::Binance.find_or_create_by!(name: 'Binance')
binance.update!(maker_fee: '0.1', taker_fee: '0.1', withdrawal_fee: '0.0002')

binance_us = Exchanges::BinanceUs.find_or_create_by!(name: 'Binance.US')
binance_us.update!(maker_fee: '0.0', taker_fee: '0.01', withdrawal_fee: '0.0002')

kraken = Exchanges::Kraken.find_or_create_by!(name: 'Kraken')
kraken.update!(maker_fee: '0.25', taker_fee: '0.4', withdrawal_fee: '0.00005')

coinbase = Exchanges::Coinbase.find_or_create_by!(name: 'Coinbase')
coinbase.update!(maker_fee: '0.6', taker_fee: '1.2', withdrawal_fee: '0.0')

bitget = Exchanges::Bitget.find_or_create_by!(name: 'Bitget')
bitget.update!(maker_fee: '0.1', taker_fee: '0.1', withdrawal_fee: '0.0')

kucoin = Exchanges::Kucoin.find_or_create_by!(name: 'KuCoin')
kucoin.update!(maker_fee: '0.1', taker_fee: '0.1', withdrawal_fee: '0.0')

bybit = Exchanges::Bybit.find_or_create_by!(name: 'Bybit')
bybit.update!(maker_fee: '0.1', taker_fee: '0.1', withdrawal_fee: '0.0')

mexc = Exchanges::Mexc.find_or_create_by!(name: 'MEXC')
mexc.update!(maker_fee: '0.0', taker_fee: '0.05', withdrawal_fee: '0.0')

gemini = Exchanges::Gemini.find_or_create_by!(name: 'Gemini')
gemini.update!(maker_fee: '0.2', taker_fee: '0.4', withdrawal_fee: '0.0')

bitvavo = Exchanges::Bitvavo.find_or_create_by!(name: 'Bitvavo')
bitvavo.update!(maker_fee: '0.15', taker_fee: '0.25', withdrawal_fee: '0.0')

# Load pre-seeded asset and ticker data from fixtures
SeedDataLoader.new.load_all

# User.find_or_create_by(email: "test@test.com") do |user|
#   user.name = "Satoshi"
#   user.password = "Polo@polo1"
#   user.confirmed_at = user.confirmed_at || Time.current
# end

# User.find_or_create_by(email: "admin@test.com") do |user|
#   user.name = "Satoshi"
#   user.password = "Polo@polo1"
#   user.confirmed_at = user.confirmed_at || Time.current
#   user.admin = true
# end

# Set default SMTP provider to env_smtp if SMTP_ADDRESS is configured
if AppConfig.smtp_env_available? && AppConfig.smtp_provider.blank?
  AppConfig.smtp_provider = 'env_smtp'
end

# Set default market data provider to deltabadger if MARKET_DATA_URL is configured
if AppConfig.market_data_env_available? && AppConfig.market_data_provider.blank?
  AppConfig.market_data_provider = MarketDataSettings::PROVIDER_DELTABADGER
end
