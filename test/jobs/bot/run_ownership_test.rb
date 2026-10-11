require 'test_helper'
require 'timeout'

class Bot::RunOwnershipTest < ActiveSupport::TestCase
  self.use_transactional_tests = false

  test 'STOP commits during a reply slower than the busy timeout and the accepted order survives restart' do
    user = create(:user, email: "ownership-#{SecureRandom.hex}@example.com")
    base = create(:asset, :bitcoin, external_id: "ownership-#{SecureRandom.hex}")
    quote = create(:asset, :usd, external_id: "ownership-#{SecureRandom.hex}")
    bot = create(:dca_single_asset, :started, user:, base_asset: base, quote_asset: quote, with_api_key: false)
    exchange = bot.exchange
    assert bot.transition_working!(:executing)
    entered = Queue.new
    release = Queue.new
    stopping = Queue.new
    stopped = Queue.new
    session_key = "ownership-session-#{SecureRandom.hex}"
    held = Queue.new
    unlock = Queue.new
    holder = Thread.new do
      ActiveRecord::Base.connection_pool.with_connection do
        IbkrLock.with_lock(session_key) do
          held << true
          unlock.pop
        end
      end
    end
    Timeout.timeout(5) { held.pop }
    bot.define_singleton_method(:market_buy) do |**|
      entered << true
      IbkrLock.with_lock(session_key, wait: 2) { true }
      sleep 6 # deliberately longer than the production five-second busy timeout
      release.pop
      Result::Success.new(order_id: 'already-transmitted')
    end
    order = { ticker: bot.ticker, side: :buy, order_type: :market_order, amount: 0.0012.to_d, quote_amount: 60.to_d, price: 50_000.to_d }
    amount = { amount: 0.0012.to_d, amount_type: :base }
    tick = Thread.new do
      ActiveRecord::Base.connection_pool.with_connection do
        result = bot.send(:create_order, order, amount)
        bot.persist_accepted_order!(order, result.data[:order_id])
      end
    end
    Timeout.timeout(5) { entered.pop }
    unlock << true
    assert holder.join(1), 'IBKR session owner must be able to release the lock'
    stop = Thread.new do
      ActiveRecord::Base.connection_pool.with_connection do
        ActiveRecord::Base.connection.execute('PRAGMA busy_timeout = 5000')
        fresh = Bot.find(bot.id)
        stopping << true
        stopped << fresh.stop
      end
    end
    Timeout.timeout(5) { stopping.pop }
    assert Timeout.timeout(1) { stopped.pop }, 'STOP must commit before the exchange replies'
    assert_equal 'stopped', Bot.find(bot.id).status
    fresh = Bot.find(bot.id)
    fresh.update_columns(status: :scheduled, transient_data: fresh.transient_data.merge('run_generation' => 99))
    release << true
    tick.value
    stop.value
    assert_equal 'scheduled', bot.reload.status
    assert bot.transactions.exists?(external_id: 'already-transmitted')
  ensure
    unlock << true if unlock
    join_for_cleanup(holder)
    release << true if release
    join_for_cleanup(tick)
    join_for_cleanup(stop)
    bot&.transactions&.delete_all
    bot&.bot_activity_logs&.delete_all
    Bot.where(id: bot&.id).delete_all
    user&.destroy!
    Ticker.where(exchange_id: exchange&.id).delete_all
    ExchangeAsset.where(asset_id: [base&.id, quote&.id]).delete_all
    Asset.where(id: [base&.id, quote&.id]).delete_all
    Exchange.where(id: exchange&.id).delete_all
  end
  test 'STOP retries SQLITE_BUSY after another short write exceeds the busy timeout' do
    user = create(:user, email: "ownership-#{SecureRandom.hex}@example.com")
    base = create(:asset, :bitcoin, external_id: "ownership-#{SecureRandom.hex}")
    quote = create(:asset, :usd, external_id: "ownership-#{SecureRandom.hex}")
    bot = create(:dca_single_asset, :started, user:, base_asset: base, quote_asset: quote, with_api_key: false)
    exchange = bot.exchange
    stopped = Queue.new
    worker = nil
    ActiveRecord::Base.transaction do
      bot.update_column(:current_delay, 1) # materialize BEGIN IMMEDIATE
      worker = Thread.new do
        ActiveRecord::Base.connection_pool.with_connection do |connection|
          connection.execute('PRAGMA busy_timeout = 20')
          stopped << Bot.find(bot.id).stop
        ensure
          connection.execute('PRAGMA busy_timeout = 5000')
        end
      end
      sleep 0.15 # at least one complete SQLite busy timeout
    end
    assert Timeout.timeout(2) { stopped.pop }
    worker.value
    assert_equal 'stopped', bot.reload.status
  ensure
    join_for_cleanup(worker)
    bot&.transactions&.delete_all
    bot&.bot_activity_logs&.delete_all
    Bot.where(id: bot&.id).delete_all
    user&.destroy!
    Ticker.where(exchange_id: exchange&.id).delete_all
    ExchangeAsset.where(asset_id: [base&.id, quote&.id]).delete_all
    Asset.where(id: [base&.id, quote&.id]).delete_all
    Exchange.where(id: exchange&.id).delete_all
  end

  test 'market clock refresh never runs inside the ownership transaction' do
    user = create(:user, email: "ownership-#{SecureRandom.hex}@example.com")
    base = create(:asset, :bitcoin, external_id: "ownership-#{SecureRandom.hex}")
    quote = create(:asset, :usd, external_id: "ownership-#{SecureRandom.hex}")
    bot = create(:dca_single_asset, :started, user:, base_asset: base, quote_asset: quote, with_api_key: false)
    exchange = bot.exchange
    bot.stubs(:ensure_exchange_authenticated)
    exchange.stubs(:market_open?).returns(false)
    reads = []
    exchange.define_singleton_method(:next_market_open_at) do |**|
      reads << ActiveRecord::Base.connection.transaction_open?
      1.hour.from_now
    end

    Bot::ActionJob.new.perform(bot)

    assert_equal [false], reads, 'fetch the venue clock once before taking the database writer'
    assert bot.reload.waiting_for_market_open
  ensure
    bot&.transactions&.delete_all
    bot&.bot_activity_logs&.delete_all
    Bot.where(id: bot&.id).delete_all
    user&.destroy!
    Ticker.where(exchange_id: exchange&.id).delete_all
    ExchangeAsset.where(asset_id: [base&.id, quote&.id]).delete_all
    Asset.where(id: [base&.id, quote&.id]).delete_all
    Exchange.where(id: exchange&.id).delete_all
  end

  private

  def join_for_cleanup(thread)
    thread&.join(10)
  rescue StandardError
    # The test observes the failure through Thread#value; still clean up its database rows.
    nil
  end
end
