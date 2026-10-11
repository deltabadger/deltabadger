require 'test_helper'
require 'timeout'

class Bot::RunOwnershipTest < ActiveSupport::TestCase
  self.use_transactional_tests = false

  test 'STOP cannot return while a venue dispatch owns the database lock' do
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
    bot.define_singleton_method(:market_buy) do |**|
      entered << true
      release.pop
      Result::Success.new(order_id: 'already-transmitted')
    end
    order = { ticker: bot.ticker, side: :buy, order_type: :market_order }
    amount = { amount: 0.0012.to_d, amount_type: :base }
    tick = Thread.new do
      ActiveRecord::Base.connection_pool.with_connection { bot.send(:create_order, order, amount) }
    end
    Timeout.timeout(5) { entered.pop }
    stop = Thread.new do
      ActiveRecord::Base.connection_pool.with_connection do
        fresh = Bot.find(bot.id)
        stopping << true
        stopped << fresh.stop
      end
    end
    Timeout.timeout(5) { stopping.pop }
    assert_raises(Timeout::Error, 'STOP must wait for the dispatch to finish') do
      Timeout.timeout(0.1) { stopped.pop }
    end
    release << true
    assert Timeout.timeout(5) { stopped.pop }
    tick.value
    stop.value
    assert_equal 'stopped', bot.reload.status
  ensure
    release << true if release
    tick&.join(5)
    stop&.join(5)
    bot&.bot_activity_logs&.delete_all
    Bot.where(id: bot&.id).delete_all
    user&.destroy!
    Ticker.where(exchange_id: exchange&.id).delete_all
    ExchangeAsset.where(asset_id: [base&.id, quote&.id]).delete_all
    Asset.where(id: [base&.id, quote&.id]).delete_all
    Exchange.where(id: exchange&.id).delete_all
  end
end
