binance = Exchanges::Binance.find_or_create_by!(name: 'Binance')
binance.update!(maker_fee: '0.1', taker_fee: '0.1', withdrawal_fee: '0.0002')

binance_us = Exchanges::BinanceUs.find_or_create_by!(name: 'Binance.US')
binance_us.update!(maker_fee: '0.0', taker_fee: '0.01', withdrawal_fee: '0.0002')

kraken = Exchanges::Kraken.find_or_create_by!(name: 'Kraken')
kraken.update!(maker_fee: '0.25', taker_fee: '0.4', withdrawal_fee: '0.00005')

coinbase = Exchanges::Coinbase.find_or_create_by!(name: 'Coinbase')
coinbase.update!(maker_fee: '0.6', taker_fee: '1.2', withdrawal_fee: '0.0')

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
