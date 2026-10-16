# Records Bot::PriceLimitable#price_limit_value_condition_satisfied? for rust/tests/conditions.rs (engine::conditions::price_met):
# a BigDecimal last price against an Integer or Float limit (and between's bounds), as the settings JSON and the venues hand
# them over. A BigDecimal never reaches the comparison: the settings JSON store hands it back as a String, which raises.
#   bin/rails runner script/rust/record_condition_vectors.rb rust/tests/fixtures/condition_vectors.json
# Re-run (script/rust/repin) when a pinned source changes.
require 'json'
require 'digest'

PORTED = %w[app/models/bot/price_limitable.rb].freeze

def encode(n)
  case n
  when Integer then { 'int' => n }
  when Float then { 'float' => [n].pack('G').unpack1('H*') }
  end
end

prices = %w[0 0.00000001 0.1 0.3 0.30000000000000004 1 99.99 100 100.00000000000001 49995.3 50000.2 123456789.12345679].map { |p| BigDecimal(p) }
limits = [0, 1, 100, 50_000, 1_000_000, 123_456_790, 0.1, 0.3, 0.30000000000000004, 1.0e-08, 99.99, 100.0, 100.00000000000001, 49_995.3,
          50_000.2, 123_456_789.12345679, 123_456_789.1234568, 1.0e-300]
bounds = [0, 100, 0.3, 0.30000000000000004, 99.99, 100.0, 50_000.2, 123_456_789.12345679]

cases = []
check = lambda do |condition, limit, lower, upper, price|
  bot = Bots::DcaMultiAsset.new(settings: { 'price_limited' => true, 'price_limit_value_condition' => condition, 'price_limit' => limit,
                                            'price_limit_range_lower_bound' => lower, 'price_limit_range_upper_bound' => upper })
  # The values as the settings store hands them back (its JSON round trip can drop a Float's 17th digit): what the comparison
  # reads, and what the engine reads from the same stored text.
  read = ->(key) { encode(bot.public_send(key)) }
  cases << { 'condition' => condition, 'limit' => read.('price_limit'), 'lower' => read.('price_limit_range_lower_bound'),
             'upper' => read.('price_limit_range_upper_bound'), 'price' => price.to_s('F'),
             'met' => bot.send(:price_limit_value_condition_satisfied?, price) }
end
%w[below above].each { |c| limits.each { |l| prices.each { |p| check.(c, l, 0, 1_000_000, p) } } }
bounds.each { |lo| bounds.each { |hi| prices.each { |p| check.('between', 1_000_000, lo, hi, p) } } }

sources = PORTED.to_h { |path| [path, Digest::SHA256.file(Rails.root.join(path)).hexdigest] }
File.write(ARGV.fetch(0), JSON.pretty_generate({ 'sources' => sources, 'cases' => cases }) + "\n")
puts "#{cases.size} condition cases"
