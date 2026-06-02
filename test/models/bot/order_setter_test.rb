require 'test_helper'

class Bot::OrderSetterTest < ActiveSupport::TestCase
  include ActiveSupport::Testing::TimeHelpers

  # Tests for order creation when amount is below minimum exchange requirements.
  # When the order amount is below the exchange's minimum, the bot should:
  # 1. Create a skipped transaction record
  # 2. NOT submit an order to the exchange
  # 3. Buffer the amount for the next interval

  # == DcaSingleAsset below minimum amount ==

  test 'single asset: creates a skipped transaction when below minimum' do
    bot = create(:dca_single_asset, :started)
    setup_bot_execution_mocks(bot, price: 50_000.0)
    bot.stubs(:broadcast_below_minimums_warning)

    assert_difference -> { bot.transactions.skipped.count }, 1 do
      bot.set_order(order_amount_in_quote: 1.0) # $1 is below minimum_quote_size of $10
    end
  end

  test 'single asset: does not call exchange API when below minimum' do
    bot = create(:dca_single_asset, :started)
    setup_bot_execution_mocks(bot, price: 50_000.0)
    bot.stubs(:broadcast_below_minimums_warning)
    bot.exchange.expects(:market_buy).never
    bot.exchange.expects(:limit_buy).never

    bot.set_order(order_amount_in_quote: 1.0)
  end

  test 'single asset: returns success when below minimum' do
    bot = create(:dca_single_asset, :started)
    setup_bot_execution_mocks(bot, price: 50_000.0)
    bot.stubs(:broadcast_below_minimums_warning)

    result = bot.set_order(order_amount_in_quote: 1.0)
    assert_kind_of Result::Success, result
  end

  test 'single asset: records skipped order with correct data' do
    bot = create(:dca_single_asset, :started)
    setup_bot_execution_mocks(bot, price: 50_000.0)
    bot.stubs(:broadcast_below_minimums_warning)

    bot.set_order(order_amount_in_quote: 1.0)

    skipped_txn = bot.transactions.skipped.last
    assert_equal 'skipped', skipped_txn.status
    assert skipped_txn.quote_amount.present?
    assert_equal 0, skipped_txn.amount_exec
    assert_equal 0, skipped_txn.quote_amount_exec
  end

  test 'single asset: does not create skipped transaction when amount meets minimum' do
    bot = create(:dca_single_asset, :started)
    setup_bot_execution_mocks(bot, price: 50_000.0)
    bot.stubs(:broadcast_below_minimums_warning)

    assert_no_difference -> { bot.transactions.skipped.count } do
      bot.set_order(order_amount_in_quote: 100.0)
    end
  end

  test 'single asset: calls exchange API when amount meets minimum' do
    bot = create(:dca_single_asset, :started)
    setup_bot_execution_mocks(bot, price: 50_000.0)
    bot.stubs(:broadcast_below_minimums_warning)
    # unstub market_buy so we can set expectation
    bot.exchange.unstub(:market_buy)
    bot.exchange.expects(:market_buy).once.returns(Result::Success.new(order_id: 'test'))

    bot.set_order(order_amount_in_quote: 100.0)
  end

  test 'single asset: returns success without transaction when amount is zero' do
    bot = create(:dca_single_asset, :started)
    setup_bot_execution_mocks(bot, price: 50_000.0)
    bot.stubs(:broadcast_below_minimums_warning)

    assert_no_difference -> { bot.transactions.count } do
      result = bot.set_order(order_amount_in_quote: 0)
      assert_kind_of Result::Success, result
    end
  end

  test 'single asset: submits order when amount is exactly at minimum' do
    bot = create(:dca_single_asset, :started)
    ticker = bot.ticker
    setup_bot_execution_mocks(bot, price: 50_000.0)
    bot.stubs(:broadcast_below_minimums_warning)
    bot.exchange.unstub(:market_buy)
    bot.exchange.expects(:market_buy).once.returns(Result::Success.new(order_id: 'test'))

    bot.set_order(order_amount_in_quote: ticker.minimum_quote_size)
  end

  test 'single asset: skips order when amount rounds below minimum after adjusting decimals' do
    bot = create(:dca_single_asset, :started)
    # minimum_base_size has more precision than base_decimals allows:
    # 0.001 requires 3 decimals, but base_decimals is only 2
    bot.ticker.update!(base_decimals: 2, minimum_base_size: 0.001)
    setup_bot_execution_mocks(bot, price: 50_000.0)
    bot.stubs(:broadcast_below_minimums_warning)
    bot.exchange.stubs(:minimum_amount_logic).returns(:base)
    bot.exchange.unstub(:market_buy)
    bot.exchange.expects(:market_buy).never

    # $70 at $50,000 = 0.0014 base → above minimum 0.001
    # but floor(0.0014, 2) = 0.00 → below minimum 0.001
    assert_difference -> { bot.transactions.skipped.count }, 1 do
      bot.set_order(order_amount_in_quote: 70)
    end
  end

  test 'single asset: skips order when base_or_quote passes quote minimum but base equivalent is below base minimum' do
    bot = create(:dca_single_asset, :started)
    # Kraken-like scenario: minimum_quote_size=0.50, minimum_base_size=0.00005
    # At price 65000, $2 quote buys 0.00003 base — below 0.00005 base minimum
    bot.ticker.update!(minimum_quote_size: 0.5, minimum_base_size: 0.00005, base_decimals: 8, quote_decimals: 5)
    setup_bot_execution_mocks(bot, price: 65_000.0)
    bot.stubs(:broadcast_below_minimums_warning)
    bot.exchange.stubs(:minimum_amount_logic).returns(:base_or_quote)
    bot.exchange.unstub(:market_buy)
    bot.exchange.expects(:market_buy).never

    assert_difference -> { bot.transactions.skipped.count }, 1 do
      bot.set_order(order_amount_in_quote: 2.0)
    end
  end

  # == Immediate persist on exchange acceptance (Finding 1) ==
  # The moment create_order succeeds the order is live on the exchange. We must
  # persist a durable Transaction (submitted / external_status unknown) right away,
  # then hand off to FetchAndUpdateOrderJob to fill in execution amounts — so a
  # failed confirmation fetch can never leave an untracked real order.

  test 'single asset: persists a submitted/unknown transaction immediately on acceptance' do
    bot = create(:dca_single_asset, :started)
    order_id = setup_bot_execution_mocks(bot, price: 50_000.0)
    bot.stubs(:broadcast_below_minimums_warning)
    Bot::FetchAndUpdateOrderJob.stubs(:perform_later)

    assert_difference -> { bot.transactions.submitted.count }, 1 do
      bot.set_order(order_amount_in_quote: 100.0)
    end

    txn = bot.transactions.order(:created_at).last
    assert_equal 'submitted', txn.status
    assert_equal 'unknown', txn.external_status
    assert_equal order_id, txn.external_id
    assert txn.quote_amount.present?
  end

  test 'single asset: enqueues FetchAndUpdateOrderJob with the persisted transaction' do
    bot = create(:dca_single_asset, :started)
    setup_bot_execution_mocks(bot, price: 50_000.0)
    bot.stubs(:broadcast_below_minimums_warning)
    Bot::FetchAndUpdateOrderJob.expects(:perform_later).with(
      instance_of(Transaction),
      update_missed_quote_amount: false
    )

    bot.set_order(order_amount_in_quote: 100.0)
  end

  test 'single asset: does not create a duplicate when external_id already exists' do
    bot = create(:dca_single_asset, :started)
    order_id = setup_bot_execution_mocks(bot, price: 50_000.0)
    bot.stubs(:broadcast_below_minimums_warning)
    Bot::FetchAndUpdateOrderJob.stubs(:perform_later)
    create(:transaction, bot: bot, status: :submitted, external_status: :unknown, external_id: order_id)

    assert_no_difference -> { bot.transactions.count } do
      bot.set_order(order_amount_in_quote: 100.0)
    end
  end

  test 'dual asset: persists submitted/unknown transactions immediately on acceptance' do
    bot = create(:dca_dual_asset, :started)
    setup_bot_execution_mocks(bot, price: 50_000.0)
    bot.stubs(:broadcast_below_minimums_warning)
    bot.stubs(:metrics).returns({ total_base0_amount: 0, total_base1_amount: 0 })
    Bot::FetchAndUpdateOrderJob.stubs(:perform_later)
    # Each leg gets a distinct exchange order id (the shared helper returns a fixed one).
    bot.exchange.unstub(:market_buy)
    bot.exchange.stubs(:market_buy)
       .returns(Result::Success.new(order_id: 'dual-0'), Result::Success.new(order_id: 'dual-1'))

    assert_difference -> { bot.transactions.submitted.where(external_status: :unknown).count }, 2 do
      bot.set_orders(total_orders_amount_in_quote: 200.0)
    end
  end

  # == Activity logging (Finding 3) ==

  test 'single asset: logs an order_skipped activity when below minimum' do
    bot = create(:dca_single_asset, :started)
    setup_bot_execution_mocks(bot, price: 50_000.0)
    bot.stubs(:broadcast_below_minimums_warning)
    Bot::FetchAndUpdateOrderJob.stubs(:perform_later)

    assert_difference -> { bot.bot_activity_logs.where(event: 'order_skipped').count }, 1 do
      bot.set_order(order_amount_in_quote: 1.0)
    end
  end

  test 'single asset: logs an order_ignored activity when the computed amount is zero' do
    bot = create(:dca_single_asset, :started)
    bot.stubs(:broadcast_below_minimums_warning)
    # Reach the defensive zero-amount branch directly (unreachable via price math for single asset).
    bot.stubs(:get_order_data).returns(
      Result::Success.new(ticker: bot.ticker, price: 50_000, amount: 0, quote_amount: 0,
                          side: :buy, order_type: :market_order)
    )

    assert_difference -> { bot.bot_activity_logs.where(event: 'order_ignored').count }, 1 do
      bot.set_order(order_amount_in_quote: 100.0)
    end
  end

  # == DcaDualAsset below minimum amount ==

  test 'dual asset: creates skipped transactions when below minimum' do
    bot = create(:dca_dual_asset, :started)
    setup_bot_execution_mocks(bot, price: 50_000.0)
    bot.stubs(:broadcast_below_minimums_warning)
    bot.stubs(:metrics).returns({ total_base0_amount: 0, total_base1_amount: 0 })

    before_count = bot.transactions.skipped.count
    bot.set_orders(total_orders_amount_in_quote: 1.0)
    after_count = bot.transactions.skipped.count
    assert after_count >= before_count + 1, 'Expected at least one skipped transaction'
  end

  test 'dual asset: does not call exchange API when below minimum' do
    bot = create(:dca_dual_asset, :started)
    setup_bot_execution_mocks(bot, price: 50_000.0)
    bot.stubs(:broadcast_below_minimums_warning)
    bot.stubs(:metrics).returns({ total_base0_amount: 0, total_base1_amount: 0 })
    bot.exchange.expects(:market_buy).never
    bot.exchange.expects(:limit_buy).never

    bot.set_orders(total_orders_amount_in_quote: 1.0)
  end

  test 'dual asset: returns success when below minimum' do
    bot = create(:dca_dual_asset, :started)
    setup_bot_execution_mocks(bot, price: 50_000.0)
    bot.stubs(:broadcast_below_minimums_warning)
    bot.stubs(:metrics).returns({ total_base0_amount: 0, total_base1_amount: 0 })

    result = bot.set_orders(total_orders_amount_in_quote: 1.0)
    assert_kind_of Result::Success, result
  end

  test 'dual asset: calls exchange API when amount meets minimum' do
    bot = create(:dca_dual_asset, :started)
    setup_bot_execution_mocks(bot, price: 50_000.0)
    bot.stubs(:broadcast_below_minimums_warning)
    bot.stubs(:metrics).returns({ total_base0_amount: 0, total_base1_amount: 0 })
    bot.exchange.unstub(:market_buy)
    bot.exchange.expects(:market_buy).at_least_once.returns(Result::Success.new(order_id: 'test'))

    bot.set_orders(total_orders_amount_in_quote: 100.0)
  end

  test 'dual asset: returns success without transaction when amount is zero' do
    bot = create(:dca_dual_asset, :started)
    setup_bot_execution_mocks(bot, price: 50_000.0)
    bot.stubs(:broadcast_below_minimums_warning)
    bot.stubs(:metrics).returns({ total_base0_amount: 0, total_base1_amount: 0 })

    assert_no_difference -> { bot.transactions.count } do
      result = bot.set_orders(total_orders_amount_in_quote: 0)
      assert_kind_of Result::Success, result
    end
  end

  # == Alpaca (quote minimum_amount_logic) ==

  test 'single asset alpaca: creates a skipped transaction when below minimum' do
    exchange = create(:alpaca_exchange)
    bot = create(:dca_single_asset, :started, exchange: exchange)
    setup_bot_execution_mocks(bot, price: 200.0)
    bot.stubs(:broadcast_below_minimums_warning)

    assert_equal :quote, exchange.minimum_amount_logic

    assert_difference -> { bot.transactions.skipped.count }, 1 do
      bot.set_order(order_amount_in_quote: 0.5) # $0.50 is below minimum_quote_size of $1
    end
  end

  test 'single asset alpaca: calls exchange API when amount meets minimum' do
    exchange = create(:alpaca_exchange)
    bot = create(:dca_single_asset, :started, exchange: exchange)
    setup_bot_execution_mocks(bot, price: 200.0)
    bot.stubs(:broadcast_below_minimums_warning)
    bot.exchange.unstub(:market_buy)
    bot.exchange.expects(:market_buy).once.returns(Result::Success.new(order_id: 'test'))

    bot.set_order(order_amount_in_quote: 100.0)
  end

  # == Buffer accumulation across intervals ==

  test 'accumulates pending amount when orders are skipped' do
    bot = build(:dca_single_asset, :started)
    bot.settings = bot.settings.merge('quote_amount' => 5.0)
    bot.set_missed_quote_amount
    bot.save!
    setup_bot_execution_mocks(bot, price: 50_000)
    bot.stubs(:broadcast_below_minimums_warning)

    initial_pending = bot.pending_quote_amount
    assert_equal 5.0, initial_pending

    bot.set_order(order_amount_in_quote: initial_pending)

    assert_equal 1, bot.transactions.skipped.count
  end

  test 'pending_quote_amount increases over multiple intervals' do
    bot = build(:dca_single_asset, :started)
    bot.settings = bot.settings.merge('quote_amount' => 5.0)
    bot.set_missed_quote_amount
    bot.save!
    setup_bot_execution_mocks(bot, price: 50_000)
    bot.stubs(:broadcast_below_minimums_warning)

    assert_equal 5.0, bot.pending_quote_amount

    travel 25.hours do
      assert_equal 10.0, bot.pending_quote_amount
    end
  end

  test 'executes order when accumulated amount reaches minimum' do
    bot = build(:dca_single_asset, :started)
    bot.settings = bot.settings.merge('quote_amount' => 5.0)
    bot.set_missed_quote_amount
    bot.save!
    setup_bot_execution_mocks(bot, price: 50_000)
    bot.stubs(:broadcast_below_minimums_warning)

    travel 25.hours do
      pending = bot.pending_quote_amount
      assert_equal 10.0, pending

      bot.exchange.unstub(:market_buy)
      bot.exchange.expects(:market_buy).once.returns(Result::Success.new(order_id: 'test'))

      bot.set_order(order_amount_in_quote: pending)

      assert_equal 0, bot.transactions.skipped.count
    end
  end

  # == Missed quote amount buffer ==

  test 'caps missed_quote_amount at effective_quote_amount on settings change' do
    bot = create(:dca_single_asset, :started)
    create(:transaction, bot: bot, quote_amount_exec: 30, external_status: :closed, created_at: Time.current)
    bot.reload

    assert_equal 70, bot.pending_quote_amount

    bot.set_missed_quote_amount
    # Capped at effective_quote_amount (100.0) — 70 < 100, so 70 is preserved
    assert_equal 70, bot.missed_quote_amount

    bot.update!(settings: bot.settings.merge('quote_amount' => 200.0))

    assert_equal 70, bot.missed_quote_amount
  end

  test 'caps missed_quote_amount when switching to smaller effective amount' do
    bot = create(:dca_single_asset, :started)
    # No transactions — pending_quote_amount equals effective_quote_amount (100.0)
    assert_equal 100.0, bot.pending_quote_amount

    # Switch to smart intervals with a much smaller effective amount
    bot.set_missed_quote_amount
    bot.update!(settings: bot.settings.merge(
      'smart_intervaled' => true,
      'smart_interval_quote_amount' => 5.0
    ))

    # missed_quote_amount should be capped at the new effective_quote_amount (5.0),
    # not carry forward the full 100.0
    assert_equal 5.0, bot.missed_quote_amount
  end

  # == String-backed limit_order_pcnt_distance regression ==

  test 'single asset: limit-order branch does not raise when limit_order_pcnt_distance is a String' do
    exchange = create(:hyperliquid_exchange)
    bot = create(:dca_single_asset, :started, exchange: exchange, with_api_key: false)
    create(:api_key, user: bot.user, exchange: exchange, key_type: :trading,
                     raw_key: "0x#{'a' * 40}", raw_secret: 'b' * 64)
    # Simulate legacy data: value persisted as String inside the JSON settings column.
    # update_columns bypasses parse_params/validations/callbacks, reproducing the production shape.
    bot.update_columns(settings: bot.settings.merge('limit_order_pcnt_distance' => '0.001'))
    bot.reload

    setup_bot_execution_mocks(bot, price: 50_000.0)
    bot.stubs(:broadcast_below_minimums_warning)
    bot.exchange.expects(:limit_buy).with do |args|
      # Expected: 50_000 * (1 - 0.001) = 49_950, then ticker.adjusted_price rounding.
      # Tolerate small rounding from price_decimals.
      price = args[:price]
      price.present? && (price.to_d - BigDecimal('49950')).abs < BigDecimal('1')
    end.returns(Result::Success.new(order_id: 'test'))

    assert_nothing_raised do
      bot.set_order(order_amount_in_quote: 100.0)
    end
  end

  # == Bitvavo limit order sizes in base (errorCode 216 regression) ==
  # Bitvavo limit orders accept only base amount + price. When BTC fell into a price
  # band where calculate_best_amount_info previously selected :quote (because Bitvavo's
  # minimum_amount_logic returned :base_or_quote for all order types), the EUR figure
  # was shipped as a BTC quantity -> Bitvavo errorCode 216 ("insufficient balance").
  # Limit orders must always be sized in base.

  test 'single asset: Bitvavo limit order is sized in base, not quote' do
    exchange = create(:bitvavo_exchange)
    bot = create(:dca_single_asset, :started, exchange: exchange,
                                              base_asset: create(:asset, :bitcoin),
                                              quote_asset: create(:asset, :eur))
    bot.ticker.update!(minimum_base_size: 0.00007989, minimum_quote_size: 5.0,
                       base_decimals: 8, price_decimals: 0)
    # Limit order with no offset, so the limit price equals the mocked price.
    bot.update_columns(settings: bot.settings.merge('limit_ordered' => true,
                                                    'limit_order_pcnt_distance' => 0))
    bot.reload

    setup_bot_execution_mocks(bot, price: 59_606.0)
    bot.stubs(:broadcast_below_minimums_warning)

    bot.exchange.expects(:limit_buy).with do |args|
      # base quantity (~0.000335 BTC), NOT the 20 EUR quote amount.
      args[:amount_type] == :base &&
        args[:amount].to_d > BigDecimal('0.0003') &&
        args[:amount].to_d < BigDecimal('0.0004')
    end.returns(Result::Success.new(order_id: 'test'))

    bot.set_order(order_amount_in_quote: 20.0)
  end

  test 'clears missed_quote_amount on bot start' do
    bot = create(:dca_single_asset, :started)
    bot.update!(missed_quote_amount: 50.0, status: :stopped)

    bot.start

    assert_equal 0, bot.missed_quote_amount
  end
end
