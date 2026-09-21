# frozen_string_literal: true

require 'test_helper'

# market_buy / market_sell with a bot_id: the same order the API always placed, now owned by a
# signal bot — a row on its page instead of nothing anywhere.
class BotApi::Orders::BotOrderTest < ActiveSupport::TestCase
  include ActiveJob::TestHelper

  def queue_adapter_for_test
    ActiveJob::QueueAdapters::TestAdapter.new
  end

  setup do
    @bot = create(:signal_bot, :started)
    @user = @bot.user
    # The service loads its own copy of the bot, so venue reads are stubbed on the classes.
    Ticker.any_instance.stubs(:get_ask_price).returns(Result::Success.new(50_000))
    Ticker.any_instance.stubs(:get_bid_price).returns(Result::Success.new(49_000))
    Exchanges::Binance.any_instance.stubs(:sleep)
  end

  def buy(**opts)
    BotApi::Orders::MarketBuy.call(user: @user, bot_id: @bot.id, amount: 100, **opts)
  end

  test 'a market buy naming a bot is recorded on that bot' do
    Exchanges::Binance.any_instance.expects(:market_buy).returns(Result::Success.new(order_id: 'api-1'))

    result = buy

    assert result.success?, result.error_message
    assert_equal :created, result.status
    txn = @bot.transactions.sole
    assert_predicate txn, :submitted?
    assert_equal 'api-1', txn.external_id
    assert_equal({ dry_run: false, bot_id: @bot.id, transaction_id: txn.id, order_id: 'api-1',
                   exchange: 'Binance', pair: 'BTC/USD', side: 'buy', order_type: 'market',
                   amount: 100, amount_type: 'quote' }, result.data)
  end

  test 'a market sell defaults to a base amount' do
    Exchanges::Binance.any_instance.expects(:market_sell).with(has_entries(amount_type: :base))
                      .returns(Result::Success.new(order_id: 'api-2'))

    result = BotApi::Orders::MarketSell.call(user: @user, bot_id: @bot.id, amount: '0.5')

    assert result.success?, result.error_message
    assert_equal 'base', result.data[:amount_type]
    assert_predicate @bot.transactions.sole, :sell?
  end

  test 'the pair comes from the bot, and a pair that contradicts it is refused' do
    Exchanges::Binance.any_instance.expects(:market_buy).never

    result = buy(exchange_name: 'Binance', base_asset: 'ETH', quote_asset: 'USD')

    assert_equal 'bot_pair_mismatch', result.error_code
    assert_equal :validation_failed, result.status
    assert_empty @bot.transactions
  end

  test 'a pair that agrees with the bot is accepted, in any case' do
    Exchanges::Binance.any_instance.stubs(:market_buy).returns(Result::Success.new(order_id: 'api-3'))

    assert buy(exchange_name: 'binance', base_asset: 'btc', quote_asset: 'usd').success?
  end

  # An exchange may list an asset under its own alias (Kraken's XBT). The API speaks asset
  # symbols everywhere else — creating the bot, reading it, its transaction rows — so a caller
  # repeating the bot's own pair must be understood, and answered in the same words.
  test 'the pair is judged and reported in asset symbols, not the exchange alias' do
    @bot.ticker.update_columns(base: 'XBT', ticker: 'XBTUSD')
    Exchanges::Binance.any_instance.stubs(:market_buy).returns(Result::Success.new(order_id: 'api-alias'))

    result = buy(exchange_name: 'Binance', base_asset: 'BTC', quote_asset: 'USD')

    assert result.success?, result.error_message
    assert_equal 'BTC/USD', result.data[:pair]
  end

  test "another user's bot is not found" do
    result = BotApi::Orders::MarketBuy.call(user: create(:user), bot_id: @bot.id, amount: 100)

    assert_equal 'bot_not_found', result.error_code
    assert_equal :not_found, result.status
  end

  test 'only a signal bot takes orders' do
    # The bot's own venue and assets: a second :bitcoin would break Asset's unique external_id.
    dca = create(:dca_single_asset, user: @user, exchange: @bot.exchange,
                                    base_asset: @bot.base_asset, quote_asset: @bot.quote_asset)

    result = BotApi::Orders::MarketBuy.call(user: @user, bot_id: dca.id, amount: 100)

    assert_equal 'bot_not_orderable', result.error_code
  end

  # The stop button in the interface is the kill switch for whatever is calling.
  test 'a stopped bot refuses the order' do
    @bot.update!(status: :stopped)
    Exchanges::Binance.any_instance.expects(:market_buy).never

    result = buy

    assert_equal 'bot_not_running', result.error_code
    assert_equal :conflict, result.status
  end

  test 'an amount that is not a positive plain number is refused' do
    ['abc', '-5', '0', '1e9', nil].each do |bad|
      result = BotApi::Orders::MarketBuy.call(user: @user, bot_id: @bot.id, amount: bad)

      assert_equal 'invalid_number', result.error_code, "amount=#{bad.inspect}"
    end
  end

  test 'an unknown amount_type is refused' do
    assert_equal 'invalid_amount_type', buy(amount_type: 'shares').error_code
  end

  test 'a missing trading key is refused' do
    @user.api_keys.destroy_all

    assert_equal 'api_key_missing', buy.error_code
  end

  # Registered with the venue but not yet activated: it must never reach a live venue call, and it
  # is not "missing" — the fix is to wait, not to add a key.
  test 'a key still pending activation is refused as pending, not as missing' do
    @user.api_keys.find_by(exchange: @bot.exchange, key_type: :trading).update_columns(status: ApiKey.statuses[:pending_activation])
    Exchanges::Binance.any_instance.expects(:market_buy).never

    result = buy

    assert_equal 'api_key_pending', result.error_code
    assert_equal :conflict, result.status
  end

  # "12abc".to_i is 12: a malformed id must not trade through a bot the caller never named.
  test 'a bot_id that is not a whole number is refused' do
    Exchanges::Binance.any_instance.expects(:market_buy).never

    ["#{@bot.id}abc", "#{@bot.id}.9", '-1', '0'].each do |bad|
      result = BotApi::Orders::MarketBuy.call(user: @user, bot_id: bad, amount: 100)

      assert_equal 'invalid_number', result.error_code, "bot_id=#{bad.inspect}"
    end
  end

  # Sent but blank is not omitted: with a complete pair beside it, reading it as "no bot" would
  # place the order on the account while the caller believes a bot owns it.
  test 'a blank bot_id beside a complete pair is refused, not placed unattributed' do
    Exchanges::Binance.any_instance.expects(:market_buy).never
    Exchanges::Binance.any_instance.expects(:limit_buy).never
    pair = { exchange_name: 'Binance', base_asset: 'BTC', quote_asset: 'USD', amount: 100 }

    ['', '   ', false].each do |blank|
      assert_equal 'invalid_number', BotApi::Orders::MarketBuy.call(user: @user, bot_id: blank, **pair).error_code,
                   "bot_id=#{blank.inspect}"
      assert_equal 'bot_limit_orders_unsupported',
                   BotApi::Orders::LimitBuy.call(user: @user, bot_id: blank, price: 40_000, **pair).error_code
    end
  end

  # JSON and MCP hand a number over as a Float, and a small Float prints as "5.0e-05" — which the
  # strict parser would refuse as something nobody typed. A small base amount is an ordinary order.
  test 'a small amount that arrived as a number is an amount, not invalid' do
    result = BotApi::Orders::MarketSell.call(user: @user, bot_id: @bot.id, amount: 0.00005)

    assert_not_equal 'invalid_number', result.error_code
  end

  test 'a whole number that arrived as a Float names the bot (MCP casts numbers to Float)' do
    Exchanges::Binance.any_instance.stubs(:market_buy).returns(Result::Success.new(order_id: 'api-f'))

    assert BotApi::Orders::MarketBuy.call(user: @user, bot_id: @bot.id.to_f, amount: 100.0).success?
  end

  # The venue holds the order: the answer is "placed", with the id to reconcile by, even though
  # there is no row to point at.
  test 'an accepted order whose row could not be written is still a 201 with the order id' do
    Exchanges::Binance.any_instance.stubs(:market_buy).returns(Result::Success.new(order_id: 'api-norow'))
    Bots::Signal.any_instance.stubs(:persist_accepted_order!).raises(ActiveRecord::StatementInvalid, 'database is locked')
    Bots::Signal.any_instance.stubs(:notify_about_error)

    result = buy

    assert result.success?, result.error_message
    assert_equal :created, result.status
    assert_equal 'api-norow', result.data[:order_id]
    assert_nil result.data[:transaction_id]
  end

  test 'an acceptance with no order id is placement_ambiguous' do
    Exchanges::Binance.any_instance.stubs(:market_buy).returns(Result::Success.new(order_id: nil))

    assert_equal 'placement_ambiguous', buy.error_code
  end

  test 'a wash-sale lock refuses a buy and lets a sell through' do
    @user.update!(wash_sale_enabled: true, wash_sale_jurisdiction: 'US')
    WashSaleLock.create!(user: @user, asset_id: @bot.ticker.base_asset_id, buy_locked_until: 10.days.from_now)
    Exchanges::Binance.any_instance.stubs(:market_sell).returns(Result::Success.new(order_id: 'api-4'))

    assert_equal 'wash_sale_locked', buy.error_code
    assert BotApi::Orders::MarketSell.call(user: @user, bot_id: @bot.id, amount: '0.5').success?
  end

  test 'a closed market refuses the order' do
    Exchanges::Binance.any_instance.stubs(:market_open?).returns(false)
    Exchanges::Binance.any_instance.expects(:market_buy).never

    result = buy

    assert_equal 'market_closed', result.error_code
    assert_equal :conflict, result.status
  end

  # Refused at the door, like a closed market, rather than recorded as a failed order on the bot.
  test 'a pair the venue no longer trades refuses the order, dry run or not' do
    @bot.ticker.update!(trading_enabled: false)
    Exchanges::Binance.any_instance.expects(:market_buy).never

    [buy, buy(dry_run: true)].each do |result|
      assert_equal 'ticker_not_tradable', result.error_code
      assert_equal :conflict, result.status
    end
    assert_empty @bot.transactions
  end

  # The MCP dry-run setting. A dry order id in the bot's ledger would be polled against the real
  # venue by the confirmation job, so a dry run validates and records nothing.
  test 'a dry run places nothing and records nothing' do
    Exchanges::Binance.any_instance.expects(:market_buy).never

    result = buy(dry_run: true)

    assert result.success?, result.error_message
    assert result.data[:dry_run]
    assert_nil result.data[:transaction_id]
    assert_empty @bot.transactions
    assert_no_enqueued_jobs
  end

  test 'below the venue minimum is a validation failure with a skipped row' do
    result = BotApi::Orders::MarketBuy.call(user: @user, bot_id: @bot.id, amount: 1)

    assert_equal 'below_minimum_amount', result.error_code
    assert_equal :validation_failed, result.status
    assert_predicate @bot.transactions.sole, :skipped?
  end

  test 'a rejection is order_failed with the venue message and a failed row' do
    Exchanges::Binance.any_instance.stubs(:market_buy).returns(Result::Failure.new('Insufficient balance.'))
    Bots::Signal.any_instance.stubs(:notify_about_error)

    result = buy

    assert_equal 'order_failed', result.error_code
    assert_equal :upstream_failed, result.status
    assert_includes result.error_message, 'Insufficient balance.'
    assert_predicate @bot.transactions.sole, :failed?
  end

  # Distinct from order_failed on purpose: a caller that retries a failure must not retry this.
  test 'an unknown outcome is placement_ambiguous and leaves no row' do
    Exchanges::Binance.any_instance.stubs(:market_buy).raises(Client::AmbiguousPlacementError, 'Net::ReadTimeout')

    result = buy

    assert_equal 'placement_ambiguous', result.error_code
    assert_equal :upstream_failed, result.status
    assert_empty @bot.transactions
  end

  test 'a limit order naming a bot is refused rather than placed unattributed' do
    Exchanges::Binance.any_instance.expects(:limit_buy).never
    Exchanges::Binance.any_instance.expects(:limit_sell).never

    [BotApi::Orders::LimitBuy, BotApi::Orders::LimitSell].each do |service|
      result = service.call(user: @user, bot_id: @bot.id, exchange_name: 'Binance', base_asset: 'BTC',
                            quote_asset: 'USD', amount: 100, price: 40_000)

      assert_equal 'bot_limit_orders_unsupported', result.error_code
      assert_equal :validation_failed, result.status
    end
  end

  test 'without a bot_id an order is placed on the account exactly as before' do
    Exchanges::Binance.any_instance.expects(:market_buy).returns(Result::Success.new(order_id: 'plain-1'))

    result = BotApi::Orders::MarketBuy.call(user: @user, exchange_name: 'Binance', base_asset: 'BTC',
                                            quote_asset: 'USD', amount: 100)

    assert result.success?, result.error_message
    assert_nil result.data[:bot_id]
    assert_empty @bot.transactions
  end
end
