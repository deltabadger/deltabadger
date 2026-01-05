binance = Exchanges::Binance.find_or_create_by!(name: 'Binance')
binance.update!(maker_fee: '0.1', taker_fee: '0.1', withdrawal_fee: '0.0002')

binance_us = Exchanges::BinanceUs.find_or_create_by!(name: 'Binance.US')
binance_us.update!(maker_fee: '0.0', taker_fee: '0.01', withdrawal_fee: '0.0002')

zonda = Exchanges::Zonda.find_or_create_by!(name: 'Zonda')
zonda.update!(maker_fee: '0.0', taker_fee: '0.1', withdrawal_fee: '0.0005')

kraken = Exchanges::Kraken.find_or_create_by!(name: 'Kraken')
kraken.update!(maker_fee: '0.25', taker_fee: '0.4', withdrawal_fee: '0.00005')

coinbase = Exchanges::Coinbase.find_or_create_by!(name: 'Coinbase')
coinbase.update!(maker_fee: '0.6', taker_fee: '1.2', withdrawal_fee: '0.0')

gemini = Exchanges::Gemini.find_or_create_by!(name: 'Gemini')
gemini.update!(maker_fee: '0.2', taker_fee: '0.4', withdrawal_fee: '0.0001')

bitso = Exchanges::Bitso.find_or_create_by!(name: 'Bitso')
bitso.update!(maker_fee: '0.5', taker_fee: '0.65', withdrawal_fee: '0.00002')

kucoin = Exchanges::Kucoin.find_or_create_by!(name: 'KuCoin')
kucoin.update!(maker_fee: '0.1', taker_fee: '0.1', withdrawal_fee: '0.0005')

bitfinex = Exchanges::Bitfinex.find_or_create_by!(name: 'Bitfinex')
bitfinex.update!(maker_fee: '0.0', taker_fee: '0.0', withdrawal_fee: '0.0004')

bitstamp = Exchanges::Bitstamp.find_or_create_by!(name: 'Bitstamp')
bitstamp.update!(maker_fee: '0.3', taker_fee: '0.4', withdrawal_fee: '0.0005')

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
