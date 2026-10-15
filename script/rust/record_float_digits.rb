# Records Float#to_d (BigDecimal(float, 0)) and Float#to_s for rust/tests/float_digits.rs.
#   bin/rails runner script/rust/record_float_digits.rb rust/tests/fixtures/float_digits.json
# Both read a double's shortest round-trip digits from dtoa mode 0, which breaks an exact tie between two shortest
# strings to the even one; to_d then truncates them to 16 digits. Seeded, so two runs write the same file. Re-run when
# Ruby or bigdecimal move (the files under "sources").
require 'json'
require 'digest'
require 'bigdecimal/util'

rng = Random.new(20_261_015)
bits = ->(f) { [f].pack('G').unpack1('H*') }
finite = lambda do
  loop do
    f = [rng.rand(2**64)].pack('Q>').unpack1('G')
    break f if f.finite?
  end
end

floats = [0.0, -0.0, 1.0, -1.0, 0.1, 0.5, 2.5, 3.0, 1.0 / 3, 0.30000000000000004, 1.0e15, 1.0e16, 1.0e17, 1.0e22, 1.0e300, 1.0e-300,
          5.0e-324, 2.2250738585072014e-308, 2.2250738585072009e-308, 1.7976931348623157e308, 9_007_199_254_740_993.0,
          123_456_789_012_345_678.0, 831_844_647_283.65625, -831_844_647_283.65625, 12_345_678.125, 1.5e-7] +
         Array.new(4000) { finite.() } +
         Array.new(2000) { rng.rand(1..10**rng.rand(1..15)).to_f / (10**rng.rand(0..12)) * (rng.rand < 0.2 ? -1 : 1) } +
         # A few binary places on a large integer part: the exact value has a short decimal expansion ending in 5, where
         # two shortest strings can be equally near.
         Array.new(4000) { rng.rand(1..2**53).to_f / (2**rng.rand(1..12)) * (rng.rand < 0.2 ? -1 : 1) } +
         Array.new(500) { rng.rand(2**53..2**63).to_f * (rng.rand < 0.2 ? -1 : 1) }

# A tie: the exact value sits halfway between two neighbours of its shortest digits.
tie = lambda do |f|
  return false if f.zero?

  shortest = BigDecimal(f.to_s)
  (f.to_r - shortest.to_r).abs * 2 == Rational(10)**(shortest.exponent - shortest.n_significant_digits)
end

rows = floats.map { |f| [bits.(f), f.to_d.to_s('F'), f.to_s, tie.(f)] }
raise 'too few ties' if rows.count(&:last) < 100

sources = %w[Gemfile.lock .ruby-version].to_h { |p| [p, Digest::SHA256.file(Rails.root.join(p)).hexdigest] }
File.write(ARGV.fetch(0), "{\n  \"sources\": #{JSON.generate(sources)},\n  \"floats\": [\n#{rows.map { |r| "    #{JSON.generate(r)}" }.join(",\n")}\n  ]\n}\n")
