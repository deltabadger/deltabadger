binance = Exchanges::Binance.find_or_create_by!(name: 'Binance')
binance.update!(maker_fee: '0.1', taker_fee: '0.1', withdrawal_fee: '0.0002')

binance_us = Exchanges::BinanceUs.find_or_create_by!(name: 'Binance.US')
binance_us.update!(maker_fee: '0.0', taker_fee: '0.01', withdrawal_fee: '0.0002')

kraken = Exchanges::Kraken.find_or_create_by!(name: 'Kraken')
kraken.update!(maker_fee: '0.25', taker_fee: '0.4', withdrawal_fee: '0.00005')

coinbase = Exchanges::Coinbase.find_or_create_by!(name: 'Coinbase')
coinbase.update!(maker_fee: '0.6', taker_fee: '1.2', withdrawal_fee: '0.0')

# Core cryptocurrencies - seeded as fallback when CoinGecko sync fails
CORE_CRYPTOCURRENCIES = [
  { external_id: 'bitcoin', symbol: 'BTC', name: 'Bitcoin' },
  { external_id: 'ethereum', symbol: 'ETH', name: 'Ethereum' },
  { external_id: 'solana', symbol: 'SOL', name: 'Solana' },
  { external_id: 'ripple', symbol: 'XRP', name: 'XRP' },
  { external_id: 'binancecoin', symbol: 'BNB', name: 'BNB' },
  { external_id: 'dogecoin', symbol: 'DOGE', name: 'Dogecoin' },
  { external_id: 'cardano', symbol: 'ADA', name: 'Cardano' },
  { external_id: 'monero', symbol: 'XMR', name: 'Monero' },
  { external_id: 'litecoin', symbol: 'LTC', name: 'Litecoin' },
  { external_id: 'pax-gold', symbol: 'PAXG', name: 'PAX Gold' }
].freeze

CORE_CRYPTOCURRENCIES.each do |crypto|
  Asset.find_or_create_by!(external_id: crypto[:external_id]) do |asset|
    asset.symbol = crypto[:symbol]
    asset.name = crypto[:name]
    asset.category = 'Cryptocurrency'
  end
end

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
