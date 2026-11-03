# Records Fiat.currencies for rust/tests/coingecko_tickers.rs (jobs::synchronizer::FIAT: the fiat assets the CoinGecko
# crawl mints and the symbols it maps to them).
#   bin/rails runner script/rust/record_fiat_currencies.rb rust/tests/fixtures/fiat_currencies.json
# Re-record when a pinned source moves (script/rust/repin).
require 'json'
require 'digest'

out = ARGV.fetch(0)
sources = %w[app/models/fiat.rb app/models/exchange/synchronizer.rb].to_h { |p| [p, Digest::SHA256.file(Rails.root.join(p)).hexdigest] }
File.write(out, "#{JSON.pretty_generate({ 'sources' => sources, 'currencies' => Fiat.currencies.map { |c| c.transform_keys(&:to_s) } })}\n")
