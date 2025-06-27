desc 'rake task with exchange implementation helpers'
task exchange_implementation_helpers: :environment do
  e = Exchange.find_by(type: 'Exchanges::Binance')
  coingecko_symbols = e.coingecko_symbols
  exchange_symbols = e.get_tickers_info(force: true).data.map { |t| [t[:base], t[:quote]] }.flatten.uniq.sort

  # File.write('coingecko.json', coingecko_symbols.to_json)
  # File.write('exchange.json', exchange_symbols.to_json)

  symbols_in_coingecko_but_not_in_exchange = coingecko_symbols - exchange_symbols
  symbols_in_exchange_but_not_in_coingecko = exchange_symbols - coingecko_symbols

  puts "symbols_in_coingecko_but_not_in_exchange: #{symbols_in_coingecko_but_not_in_exchange}"
  puts "symbols_in_exchange_but_not_in_coingecko: #{symbols_in_exchange_but_not_in_coingecko}"
end

# Example output for kraken:
# symbols_in_coingecko_but_not_in_exchange = coingecko_symbols - exchange_symbols
# => ["DOGE"]
# symbols_in_exchange_but_not_in_coingecko = exchange_symbols - coingecko_symbols
# => ["AURA", "REP", "SPK", "XDG"]
#
# That means
# a) we will ignore "AURA", "REP", "SPK" because we lack data for them on coingecko.
# b) we have to map coigecko's "DOGE" to "XDG" to match exchange.

# Example output for coinbase:
# symbols_in_coingecko_but_not_in_exchange = coingecko_symbols - exchange_symbols
# => ["PIRATE"]
# symbols_in_exchange_but_not_in_coingecko = exchange_symbols - coingecko_symbols
# => ["RENDER", "WAXL", "ZETACHAIN"]
#
# That means
# a) we will ignore "RENDER", "WAXL", "ZETACHAIN" because we lack data for them on coingecko.
# b) we will ignore "PIRATE" because it's not listed on the exchange.
