require 'test_helper'

# A pair the venue no longer trades keeps its row: bots and trade history point at it, so the sync
# marks it unavailable instead of deleting it. That row is still found by the pair lookup, and must
# be refused before anything is sent to the venue — the same test a bot has to pass to start.
class BotApi::Orders::UntradableTickerTest < ActiveSupport::TestCase
  def setup
    @user = create(:user)
    @exchange = create(:binance_exchange)
    create(:api_key, user: @user, exchange: @exchange, key_type: :trading, status: :correct)
    @asset = create(:asset, symbol: 'AAA', name: 'Coin AAA', external_id: 'coin-aaa')
    @quote = Asset.find_by(symbol: 'USDT') || create(:asset, symbol: 'USDT', name: 'Tether', external_id: 'tether')
    @ticker = create(:ticker, exchange: @exchange, base_asset: @asset, quote_asset: @quote)
  end

  def order(service, **opts)
    service.call(user: @user, exchange_name: @exchange.name, base_asset: 'AAA', quote_asset: @quote.symbol,
                 amount: '10', **opts)
  end

  { 'unavailable' => { available: false }, 'trading-disabled' => { trading_enabled: false } }.each do |label, attrs|
    test "a market buy on a #{label} pair is refused without reaching the exchange" do
      @ticker.update!(attrs)
      Exchanges::Binance.any_instance.expects(:market_buy).never

      result = order(BotApi::Orders::MarketBuy, amount_type: 'quote')

      assert_not result.success?
      assert_equal 'ticker_not_tradable', result.error_code
      assert_equal :conflict, result.status
      assert_match(%r{AAA/USDT}, result.error_message)
    end

    test "a market sell on a #{label} pair is refused without reaching the exchange" do
      @ticker.update!(attrs)
      Exchanges::Binance.any_instance.expects(:market_sell).never

      result = order(BotApi::Orders::MarketSell, amount_type: 'base')

      assert_equal 'ticker_not_tradable', result.error_code
    end
  end

  test 'limit orders on an untradable pair are refused too' do
    @ticker.update!(available: false)
    Exchanges::Binance.any_instance.expects(:limit_buy).never
    Exchanges::Binance.any_instance.expects(:limit_sell).never

    assert_equal 'ticker_not_tradable', order(BotApi::Orders::LimitBuy, price: '1').error_code
    assert_equal 'ticker_not_tradable', order(BotApi::Orders::LimitSell, price: '1').error_code
  end

  test 'a pair with no row at all is still not found' do
    result = BotApi::Orders::MarketBuy.call(user: @user, exchange_name: @exchange.name, base_asset: 'ZZZ',
                                            quote_asset: @quote.symbol, amount: '10')

    assert_equal 'pair_not_found', result.error_code
  end

  test 'a tradable pair is placed' do
    Exchanges::Binance.any_instance.stubs(:market_buy).returns(Result::Success.new(order_id: 'x'))

    assert_predicate order(BotApi::Orders::MarketBuy, amount_type: 'quote'), :success?
  end
end
