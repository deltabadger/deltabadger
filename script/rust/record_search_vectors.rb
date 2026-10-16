# Records what the asset search (app/controllers/concerns/bots/searchable.rb) computes in Ruby, for the Rust port's tests:
#   bin/rails runner script/rust/record_search_vectors.rb rust/tests/fixtures/search_vectors.json
# - "jaro_winkler": [a, b, JaroWinkler.similarity(a, b)] with the gem's default options, as the search calls it. Hand-picked
#   pairs (empty, case, unicode, transpositions, the prefix bonus), then seeded random ones, then random pairs that land near
#   the cut-offs (0.85 for assets, 0.7 for exchanges, also the gem's own prefix-bonus threshold).
# - "downcase": [s, s.downcase]. The search downcases the query and every symbol and name before comparing; Ruby's full
#   Unicode mapping is not every language's (final sigma, dotted I).
# - "next_page_query": [raw query string, request.query_parameters.merge('offset' => 20).to_query], the lazy frame's src.
# Seeded, so two runs write the same file. Re-run when the gem, Ruby or searchable.rb move (the files under "sources").
require 'json'
require 'digest'

rng = Random.new(20_261_016)
jw = ->(a, b) { JaroWinkler.similarity(a, b) }
alphabet = [*'a'..'z', *'A'..'Z', *'0'..'9', ' ', '.', '-', 'é', 'ü', 'ß', 'ø', 'ł', 'ś', 'Σ', 'σ', 'ж', '日', '本', '€', '🚀']
word = ->(max) { Array.new(rng.rand(0..max)) { alphabet[rng.rand(alphabet.size)] }.join }
# A string and a mutation of it: what a mistyped search looks like.
typo = lambda do |s|
  chars = s.chars
  rng.rand(1..3).times do
    i = chars.empty? ? 0 : rng.rand(chars.size)
    case rng.rand(4)
    when 0 then chars.insert(i, alphabet[rng.rand(26)])
    when 1 then chars.delete_at(i)
    when 2 then chars[i] = alphabet[rng.rand(26)] unless chars.empty?
    else chars[i], chars[i + 1] = chars[i + 1], chars[i] if i + 1 < chars.size
    end
  end
  chars.join
end

names = %w[bitcoin ethereum solana tether cardano polkadot dogecoin avalanche chainlink litecoin nvda nvidia apple microsoft
           amazon meta alphabet tesla broadcom costco netflix kraken binance coinbase bitstamp gemini bitvavo alpaca hyperliquid]
pairs = [['', ''], ['', 'a'], ['a', ''], ['a', 'a'], ['a', 'b'], ['ab', 'ba'], %w[martha marhta], %w[dixon dicksonx], %w[jellyfish smellyfish],
         %w[dwayne duane], %w[crate trace], %w[abcd abdc], %w[nvda nvdia], %w[eth etherum], %w[solana solano], %w[btc bitcoin],
         %w[ETH eth], %w[Bitcoin bitcoin], %w[éth eth], %w[über uber], %w[straße strasse], %w[日本 日本語], %w[🚀 🚀🚀], %w[σας σασ],
         ['a b', 'ab'], %w[aaaa aaab], %w[abcdefgh abcdefgx], %w[x xxxxxxxxxxxxxxxx], %w[kraken krakken], %w[binance binnace],
         %w[aaaaaaaaaa aaaaaaaaab], %w[ab abcdefghij], %w[abcdefghij ab], %w[1inch 1inch], %w[usdt usdc]]
names.each { |n| pairs << [n, typo.(n)] << [n, n[0, rng.rand(1..n.size)]] << [typo.(n), n] }
pairs += Array.new(260) { [word.(10), word.(10)] }
near = { 0.85 => [], 0.7 => [] }
200_000.times do
  break if near.values.all? { |found| found.size >= 60 }

  base = rng.rand < 0.5 ? names.sample(random: rng) : word.(12)
  pair = [base, typo.(base)]
  cut = near.keys.find { |c| (jw.(*pair) - c).abs <= 0.02 }
  near[cut] << pair if cut && near[cut].size < 60
end
raise "too few pairs near the cut-offs: #{near.transform_values(&:size)}" if near.values.any? { |found| found.size < 60 }

pairs += near.values.flatten(1)
rows = pairs.uniq.map { |a, b| [a, b, jw.(a, b)] }

downcase = (%w[BTC Bitcoin ÉTH ÜBER STRASSE ẞ ΣΑΣ ΟΔΟΣ İstanbul I Ǆ ǅ ΣΑΣ. Ⅻ ＡＢＣ Ꭰ ﬀ] + names.map(&:upcase) +
            Array.new(40) { word.(12) }).uniq.map { |s| [s, s.downcase] }

queries = ['asset_field=add_asset_id', 'asset_field=add_asset_id&query=et', 'query=et&asset_field=add_asset_id', 'asset_field=add_asset_id&query=',
           'asset_field=add_asset_id&offset=20', 'offset=5&query=x', 'query=a+b', 'query=a%20b', 'query=%C3%A9t', 'query=%E6%97%A5', 'query=a%26b',
           'query=a%3Db', 'query=a/b', 'query=a~b*c', "query=a'b", 'query=a&query=b', 'B=1&a=2', 'b=1&a=2&a2=3', 'a=1&a-b=2&a_b=3&a.b=4',
           'query=%F0%9F%9A%80', 'query', '', 'a[]=1&a[]=2', 'a[b]=1&a[c]=2', 'query=x&utf8=%E2%9C%93', 'asset_field=quote_asset_id&query=US']
query_rows = queries.map do |raw|
  [raw, Rack::Utils.parse_nested_query(raw).merge('offset' => 20).to_query]
end
# The request path is what Rails parses; this checks the parser above is the one the controller reads.
app = ActionDispatch::Request.new(Rack::MockRequest.env_for('/x?query=a+b&B=1'))
raise 'query_parameters parse differently' unless app.query_parameters == Rack::Utils.parse_nested_query('query=a+b&B=1')

sources = %w[Gemfile.lock .ruby-version app/controllers/concerns/bots/searchable.rb].to_h { |p| [p, Digest::SHA256.file(Rails.root.join(p)).hexdigest] }
lines = ->(list) { list.map { |r| "    #{JSON.generate(r)}" }.join(",\n") }
File.write(ARGV.fetch(0), "{\n  \"sources\": #{JSON.generate(sources)},\n  \"jaro_winkler\": [\n#{lines.(rows)}\n  ],\n" \
                          "  \"downcase\": [\n#{lines.(downcase)}\n  ],\n  \"next_page_query\": [\n#{lines.(query_rows)}\n  ]\n}\n")
