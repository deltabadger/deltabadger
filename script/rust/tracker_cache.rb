# Record the real Rails ledger-cache lifecycle; no network and no retained application rows.
require 'digest'
ENV['TZ'] = 'UTC'
require 'active_support/testing/time_helpers'
clock = Object.new.extend(ActiveSupport::Testing::TimeHelpers)
Net::HTTP.prepend(Module.new { def connect = raise('unexpected network in cache oracle') })
Rails.cache = ActiveSupport::Cache::MemoryStore.new
ActiveJob::Base.queue_adapter = :test
states = {}
ActiveRecord::Base.transaction do
  clock.travel_to(Time.utc(2026, 10, 8, 12)) do
    user = User.new(email: 'cache-owner@example.test', password: 'test-password-123', confirmed_at: Time.current)
    user.save!(validate: false)
    other = User.new(email: 'cache-other@example.test', password: 'test-password-123', confirmed_at: Time.current)
    other.save!(validate: false)
    exchange = Exchanges::Alpaca.create!(name: 'Alpaca')
    row = { exchange: exchange, entry_type: :deposit, base_currency: 'USD', base_amount: 100, transacted_at: Time.current }
    read = -> { Tracker::Ledger.cached(user) ? 'warm' : 'cold' }
    warm = -> { Tracker::Ledger.compute!(user) }
    states['absent'] = read.call
    warm.call
    states['computed'] = read.call
    states['other_owner'] = Tracker::Ledger.cached(other) ? 'warm' : 'cold'
    states['other_scope'] = Tracker::Ledger.cached(user, exchange: Struct.new(:id).new(-1)).total_invested_usd.to_s('F')
    AccountTransaction.create!(**row, user: other)
    states['foreign_transaction'] = read.call
    tx = AccountTransaction.create!(**row, user: user)
    states['insert'] = read.call
    warm.call
    tx.update_columns(updated_at: Time.current + 1.second)
    states['update'] = read.call
    warm.call
    tx.destroy!
    states['delete'] = read.call
    warm.call
    HistoricalPrice.create!(asset: 'unused', currency: 'USD', date: Date.current, price: 1)
    states['price'] = read.call
    warm.call
    clock.travel(30.days - 1.second)
    states['before_expiry'] = read.call
    clock.travel(1.second)
    states['expiry'] = read.call
    clock.travel(-30.days)
    Rails.cache.write(Tracker::Ledger.send(:cache_key, user), 'wrong shape')
    states['malformed'] = read.call
  end
  raise ActiveRecord::Rollback
end
sources = %w[app/models/tracker/ledger.rb app/models/historical_price.rb app/jobs/tracker/ledger_job.rb].to_h do |path|
  [path, Digest::SHA256.file(Rails.root.join(path)).hexdigest]
end
File.write(ARGV.fetch(0), "#{JSON.pretty_generate({ sources: sources, states: states })}\n")
