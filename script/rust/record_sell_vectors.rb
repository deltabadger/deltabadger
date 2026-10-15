# Records a selling bot's schedule for rust/tests/schedule.rs, from Rails' own methods on unsaved bots.
#   bin/rails runner script/rust/record_sell_vectors.rb rust/tests/fixtures/sell_vectors.json
# Bot::Reversible#effective_interval_duration divides a Duration by (BigDecimal / Float), so the sell cadence is a
# BigDecimal-valued Duration; the engine's one duration function must land on the very checkpoints this records.
# Re-run (script/rust/repin) when a pinned source changes.
require 'json'
require 'digest'
require 'active_support/testing/time_helpers'
include ActiveSupport::Testing::TimeHelpers

PORTED = %w[app/models/bot/reversible.rb app/models/automation/schedulable.rb app/models/bot/smart_intervalable.rb
            app/models/bot/lifecycle.rb].freeze

anchors = [Time.utc(2026, 1, 31, 10, 0, 0, 123_456), Time.utc(2026, 9, 1, 22, 1, 3), Time.utc(2026, 2, 28, 23, 59, 59, 999_999)]
# A one-asset basket: a wider one (or a missing denomination) sells for N quote, which Smart Intervals never splits.
base = { 'interval' => 'day', 'quote_amount' => 10.0, 'direction' => 'selling', 'allocations' => { '1' => 1.0 }, 'sell_denomination' => 'base' }
settings = [
  {}, { 'sell_interval' => 'hour' }, { 'sell_interval' => 'week' }, { 'sell_interval' => 'month' },
  { 'interval' => 'week' }, # no sell_interval: the buy interval
  { 'sell_amount' => 0.01 }, { 'sell_denomination' => 'quote', 'sell_quote_amount' => 50.0 }
]
splits = [[0.01, 0.001], [0.3, 0.1], [1, 0.07], [7, 1], [0.123456789, 0.0137], [0.5, 1.0 / 3], [2.5, 0.6], [0.000123, 0.0000041],
          [10, 10], [1, 0.3], [100, 3], [0.7, 0.0001]]
%w[hour day week month].each do |iv|
  splits.each do |amount, split|
    settings << { 'sell_interval' => iv, 'sell_amount' => amount, 'smart_intervaled' => true, 'smart_interval_base_amount' => split }
  end
end
# The split is off, or the sentence is quote-denominated, or the amount is blank: the plain sell interval.
settings << { 'sell_interval' => 'week', 'sell_amount' => 1, 'smart_intervaled' => false, 'smart_interval_base_amount' => 0.3 }
settings << { 'sell_interval' => 'week', 'sell_denomination' => 'quote', 'sell_amount' => 1, 'smart_intervaled' => true, 'smart_interval_base_amount' => 0.3 }
settings << { 'sell_interval' => 'week', 'smart_intervaled' => true, 'smart_interval_base_amount' => 0.3 }
settings << { 'sell_interval' => 'week', 'sell_denomination' => nil, 'sell_amount' => 1, 'smart_intervaled' => true, 'smart_interval_base_amount' => 0.3 }
settings << { 'sell_interval' => 'week', 'allocations' => { '1' => 0.5, '2' => 0.5 }, 'sell_amount' => 1, 'smart_intervaled' => true, 'smart_interval_base_amount' => 0.3 }
settings << { 'sell_interval' => 'week', 'allocations' => nil, 'base_asset_ids' => [1], 'sell_amount' => 1, 'smart_intervaled' => true, 'smart_interval_base_amount' => 0.3 }
settings << { 'sell_interval' => 'hour', 'sell_amount' => '0.5', 'smart_intervaled' => true, 'smart_interval_base_amount' => '0.2' }
rng = Random.new(6061)
200.times do
  settings << { 'sell_interval' => %w[hour day week month].sample(random: rng), 'sell_amount' => (rng.rand(1..99_999) / 10.0**rng.rand(0..6)),
                'smart_intervaled' => true, 'smart_interval_base_amount' => (rng.rand(1..9_999) / 10.0**rng.rand(0..8)) }
end
offsets = [-3600, 0, 1, 3599.9999995, 86_400 * 3 + 7, 86_400 * 45, 86_400 * 400 + 0.5]

cases = []
record = lambda do |anchor, s, now, bot|
  travel_to(now, with_usec: true) do
    nxt = bot.next_interval_checkpoint_at
    cases << { 'anchor' => anchor.iso8601(6), 'now' => now.iso8601(6), 'settings' => s, 'seconds' => bot.effective_interval_duration.to_f.to_s,
               'month' => bot.effective_interval_duration == 1.month,
               'next' => nxt.round(6).iso8601(6), 'last' => bot.last_interval_checkpoint_at.round(6).iso8601(6) }
    nxt
  end
end
settings.each_with_index do |extra, i|
  s = base.merge(extra)
  anchor = anchors[i % anchors.size]
  bot = Bots::DcaMultiAsset.new(started_at: anchor, settings: s)
  offsets.each { |off| record.call(anchor, s, anchor + off, bot) }
  grid = []
  point = anchor
  3.times { point = travel_to(point + Rational(1, 1_000_000), with_usec: true) { bot.next_interval_checkpoint_at.round(6) }; grid << point }
  grid.each { |g| [-1, 0, 1].each { |d| record.call(anchor, s, g + Rational(d, 1_000_000), bot) } }
end

# Bot::Lifecycle#restarting_within_interval? for a stopped selling bot: elapsed since its last tick vs the sell cadence.
restarts = []
settings.first(40).each_with_index do |extra, i|
  s = base.merge(extra)
  anchor = anchors[i % anchors.size]
  bot = Bots::DcaMultiAsset.new(started_at: anchor, settings: s, status: :stopped)
  seconds = bot.effective_interval_duration.to_f
  [0.5, 1 - 1e-9, 1, 1 + 1e-9, 3].each do |f|
    last = anchor + 3600
    bot.last_action_job_at = last.iso8601(6)
    now = last + Rational((seconds * f * 1_000_000).round, 1_000_000)
    travel_to(now, with_usec: true) do
      restarts << { 'settings' => s, 'last_action_job_at' => last.iso8601(6), 'now' => now.iso8601(6), 'within' => bot.restarting_within_interval? }
    end
  end
end

sources = PORTED.to_h { |path| [path, Digest::SHA256.file(Rails.root.join(path)).hexdigest] }
File.write(ARGV.fetch(0), JSON.pretty_generate({ 'sources' => sources, 'schedule' => cases, 'restart' => restarts }) + "\n")
puts "#{cases.size} schedule cases, #{restarts.size} restart cases"
