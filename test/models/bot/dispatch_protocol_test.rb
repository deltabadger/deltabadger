require 'test_helper'

class Bot::DispatchProtocolTest < ActiveSupport::TestCase
  test 'STOP advances generation even without restarting' do
    bot = create(:dca_single_asset, :started)
    generation = bot.run_identity.first
    assert bot.stop
    assert_equal generation + 1, bot.reload.run_identity.first
  end

  test 'all captured transient writes refuse a superseded run' do
    bot = create(:dca_single_asset, :started)
    bot.capture_run!
    Bot.find(bot.id).stop
    Bot.find(bot.id).start(start_fresh: false)
    expected = Bot.find(bot.id).transient_data
    bot.update_columns(transient_data: {})
    bot.merge_transient_data!('price_limit_condition_met_at' => Time.current)
    assert_equal expected, Bot.find(bot.id).transient_data
  end

  test 'dispatch commits an intent and attributes the accepted row to its original run' do
    bot = create(:dca_single_asset, :started)
    bot.capture_run!
    identity = bot.run_identity
    bot.define_singleton_method(:market_buy) do |**|
      raise 'no committed intent' if Bot.find(id).placement_intents.empty?

      Bot.find(id).stop
      Bot.find(id).start(start_fresh: false)
      Result::Success.new(order_id: 'accepted-before-stop')
    end
    order = { ticker: bot.ticker, side: :buy, order_type: :market_order, amount: 0.0012.to_d, quote_amount: 60.to_d }
    result = bot.send(:create_order, order, { amount: 60, amount_type: :quote })
    assert bot.transactions.exists?(external_id: 'accepted-before-stop'), 'record before returning control to any job'
    row = bot.persist_accepted_order!(order, result.data[:order_id])
    assert_equal identity, row.run_identity
    assert_operator row.created_at, :<=, Bot.find(bot.id).stopped_at, 'date the order when its original run sent it'
    assert_empty Bot.find(bot.id).placement_intents
  end
  test 'a stale signal placement is fenced after STOP and resume' do
    bot = create(:signal_bot, :started)
    Bot.find(bot.id).stop
    Bot.find(bot.id).start(start_fresh: false)
    bot.expects(:market_buy).never
    order = { ticker: bot.ticker, side: :buy, order_type: :market_order }
    assert_same Automation::Statusable::FENCED, bot.send(:create_order, order, { amount: 60, amount_type: :quote })
  end

  test 'price reply after resume cannot restore the old trigger data' do
    bot = create(:dca_single_asset, :started)
    bot.capture_run!
    ticker = bot.ticker
    bot.stubs(:tickers).returns(stub(available: stub(find_by: ticker)))
    bot.stubs(:active_price_limited?).returns(true)
    bot.stubs(:price_limit_timing_condition_satisfied?).returns(false)
    bot.stubs(:price_limit_value_condition_satisfied?).returns(true)
    before = nil
    ticker.define_singleton_method(:get_last_price) do
      unless before
        Bot.find(bot.id).stop
        Bot.find(bot.id).start(start_fresh: false)
        before = Bot.find(bot.id).attributes
      end
      Result::Success.new(50_000.to_d)
    end
    begin
      bot.get_price_limit_condition_met?
    rescue ActiveRecord::RecordNotSaved
      # The save refuses the stale captured run.
    end
    assert_equal before, Bot.find(bot.id).attributes
  end
  test 'a stale failed price read cannot write a failed order or activity' do
    bot = create(:dca_single_asset, :started)
    bot.capture_run!
    Bot.find(bot.id).stop
    Bot.find(bot.id).start(start_fresh: false)
    count = bot.bot_activity_logs.count
    bot.create_failed_order!(ticker: bot.ticker, error_messages: ['price unavailable'])
    bot.log_activity('old_tick_failed')
    assert_empty bot.transactions
    assert_equal count, bot.bot_activity_logs.count
  end
end
