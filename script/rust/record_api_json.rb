# Records how the REST API's `render json:` writes a value (rust/src/web/rest.rs `encode`, its test):
#   bin/rails runner script/rust/record_api_json.rb rust/tests/fixtures/api_json_vectors.json
# Each vector is a value of one kind and the bytes `{ data: value, error: nil }.to_json` gives for it: what
# Api::V1::BaseController#render_result renders, through Oj.optimize_rails (config/initializers/oj.rb). Floats travel
# as their bits; decimals, integers and times as text. Re-run when Oj, Rails or Ruby move (the files under "sources").
require 'json'
require 'digest'
require 'bigdecimal'

bits = ->(f) { [f].pack('G').unpack1('H*') }
rng = Random.new(20_261_017)
floats = [0.0, -0.0, 1.0, -1.0, 0.1, 0.5, 100.0, 1.0 / 3, 0.30000000000000004, 1.0e-4, 1.0e-5, 1.5e-7, 1.0e15, 1.0e16, 1.0e17,
          1.0e20, 1.0e22, 1.0e300, 1.0e-300, 5.0e-324, 2.2250738585072014e-308, Float::MAX, -Float::MAX, Float::MIN,
          9_007_199_254_740_993.0, 123_456_789.123, 1234567890123456.7, 0.1 + 0.2, 1.0000000000000002, 99.99999999999999] +
         Array.new(300) do
           loop do
             f = [rng.rand(2**64)].pack('Q>').unpack1('G')
             break f if f.finite?
           end
         end +
         Array.new(300) { rng.rand(1..10**rng.rand(1..15)).to_f / (10**rng.rand(0..12)) * (rng.rand < 0.2 ? -1 : 1) }
integers = [0, 1, -1, 42, 2**31, 2**53, 2**53 + 1, 2**62, 2**63 - 1, -(2**63), 2**64 - 1]
decimals = %w[0 -0 0.0 1 -1 0.1 0.01 5 5.0 123.4500 0.00000001 1e-20 1e20 1E+30 -1.5e-9 123456789012345678901234567890.123456789
              0.000000000000000000000000000001 3.14159265358979323846264338327950288]
strings = ['', 'plain', '<script>alert("x")</script>', 'a & b > c < d', "  ", 'é 日本 🦡', "\u0000\u0001\u001f\u007f",
           "\" \\ / \b \f \n \r \t", " ​﻿", 'ü' * 3]
times = ['2026-09-10T12:00:30.123456Z', '2026-01-01T00:00:00Z', '1999-12-31T23:59:59.999999Z', '2026-09-10T12:00:30.000999Z']

render = ->(value) { { data: value, error: nil }.to_json }
vectors = floats.map { |f| ['float', bits.(f), render.(f)] } +
          integers.map { |i| ['integer', i.to_s, render.(i)] } +
          decimals.map { |d| ['decimal', d, render.(BigDecimal(d))] } +
          strings.map { |s| ['string', s, render.(s)] } +
          # A datetime column read back: an ActiveSupport::TimeWithZone in the app's zone (UTC).
          times.map { |t| ['time', t, render.(Time.iso8601(t).in_time_zone)] } +
          [['nil', '', render.(nil)], ['true', '', render.(true)], ['false', '', render.(false)],
           ['object', '', render.({ b: 1, a: [nil, 'x', 2.5], 'c' => { z: BigDecimal('1.10') } })]]

sources = %w[Gemfile.lock .ruby-version config/initializers/oj.rb app/controllers/api/v1/base_controller.rb]
          .to_h { |p| [p, Digest::SHA256.file(Rails.root.join(p)).hexdigest] }
File.write(ARGV.fetch(0), "{\n  \"sources\": #{JSON.generate(sources)},\n  \"vectors\": [\n" \
                          "#{vectors.map { |v| "    #{JSON.generate(v)}" }.join(",\n")}\n  ]\n}\n")
