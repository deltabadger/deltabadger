require 'test_helper'

# A direct buy has no bot and no schedule, so there is nothing to carry forward and silence would be
# wrong: the caller asked for a buy and must be told why it did not happen.
class BotApi::Orders::WashSaleTest < ActiveSupport::TestCase
  def setup
    @user = create(:user)
    @exchange = create(:binance_exchange)
    create(:api_key, user: @user, exchange: @exchange, key_type: :trading, status: :correct)
    @asset = create(:asset, symbol: 'AAA', name: 'Coin AAA', external_id: 'coin-aaa')
    @quote = Asset.find_by(symbol: 'USDT') || create(:asset, symbol: 'USDT', name: 'Tether', external_id: 'tether')
    @ticker = create(:ticker, exchange: @exchange, base_asset: @asset, quote_asset: @quote)
    @user.update!(wash_sale_enabled: true, wash_sale_jurisdiction: 'US')
  end

  def lock_it
    WashSaleLock.create!(user: @user, asset: @asset, buy_locked_until: Time.zone.parse('2026-10-08 00:00'))
  end

  def market_buy
    BotApi::Orders::MarketBuy.call(user: @user, exchange_name: @exchange.name, base_asset: 'AAA',
                                   quote_asset: @quote.symbol, amount: '10', amount_type: 'quote')
  end

  test 'a market buy of a locked asset is refused, named and dated' do
    lock_it
    Exchanges::Binance.any_instance.expects(:market_buy).never

    result = market_buy

    assert_not result.success?
    assert_equal 'wash_sale_locked', result.error_code
    assert_match(/AAA/, result.error_message)
    assert_match(/2026-10-07/, result.error_message, 'the last locked day, not the day buying resumes')
  end

  test 'a limit buy of a locked asset is refused too' do
    lock_it
    Exchanges::Binance.any_instance.expects(:limit_buy).never

    result = BotApi::Orders::LimitBuy.call(user: @user, exchange_name: @exchange.name, base_asset: 'AAA',
                                           quote_asset: @quote.symbol, amount: '10', price: '1')

    assert_not result.success?
    assert_equal 'wash_sale_locked', result.error_code
  end

  test 'the rule being off lets the buy through, though the lock row survives' do
    lock_it
    @user.update!(wash_sale_enabled: false)
    Exchanges::Binance.any_instance.stubs(:market_buy).returns(Result::Success.new(order_id: 'x'))

    assert_predicate market_buy, :success?
    assert_equal 1, @user.wash_sale_locks.count, 'switching off releases buying without forgetting the window'
  end

  test 'an unlocked asset is unaffected' do
    Exchanges::Binance.any_instance.stubs(:market_buy).returns(Result::Success.new(order_id: 'x'))

    assert_predicate market_buy, :success?
  end

  test 'a SELL of a locked asset is never refused' do
    lock_it
    Exchanges::Binance.any_instance.stubs(:market_sell).returns(Result::Success.new(order_id: 'x'))

    result = BotApi::Orders::MarketSell.call(user: @user, exchange_name: @exchange.name, base_asset: 'AAA',
                                             quote_asset: @quote.symbol, amount: '10', amount_type: 'base')

    assert_predicate result, :success?, 'the window blocks buying, never selling'
  end
end
