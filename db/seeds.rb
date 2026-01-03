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
