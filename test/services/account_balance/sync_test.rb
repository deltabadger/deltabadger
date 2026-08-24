require 'test_helper'

class AccountBalance::SyncTest < ActiveSupport::TestCase
  setup do
    @user = create(:user)
    @exchange = create(:binance_exchange)
    @api_key = create(:api_key, user: @user, exchange: @exchange)
    @btc = create(:asset, :bitcoin)
    @eth = create(:asset, :ethereum)

    @exchange.stubs(:set_client)
  end

  test 'creates balance rows with fresh USD pricing' do
    @exchange.stubs(:get_balances).returns(Result::Success.new(
                                             @btc.id => { free: 0.5.to_d, locked: 0 },
                                             @eth.id => { free: 2.to_d, locked: 1.to_d }
                                           ))
    MarketData.stubs(:get_prices).returns(Result::Success.new(
                                            'bitcoin' => 50_000.0, 'ethereum' => 3_000.0
                                          ))

    result = AccountBalance::Sync.new(@api_key).sync!

    assert result.success?
    summary = result.data
    assert_equal 2, summary.synced
    assert_equal 2, summary.priced_fresh
    assert_equal 0, summary.priced_stale
    assert_equal 0, summary.unpriced
    assert_nil summary.pricing_error

    btc_bal = AccountBalance.find_by(user: @user, asset: @btc)
    assert_equal 50_000.to_d, btc_bal.usd_price
    assert_equal (0.5.to_d * 50_000.to_d), btc_bal.usd_value
    assert_not_nil btc_bal.priced_at
  end

  # A dollar is worth a dollar. `usd` is ALSO a CoinGecko coin id — a micro-cap token — so a cash
  # balance looked up like any other asset came back at a fraction of a cent, and took the whole
  # account's value down with it.
  test 'cash is valued at its exchange rate, never through the market feed' do
    usd = create(:asset, :usd)
    @exchange.stubs(:get_balances).returns(Result::Success.new(
                                             usd.id => { free: 84_914.to_d, locked: 0 },
                                             @btc.id => { free: 1.to_d, locked: 0 }
                                           ))
    # The feed is asked about bitcoin and nothing else: a coin id that happens to spell a currency
    # must never reach it.
    MarketData.expects(:get_prices).with(coin_ids: ['bitcoin'], currency: 'usd')
              .returns(Result::Success.new('bitcoin' => 50_000.0))

    AccountBalance::Sync.new(@api_key).sync!

    cash = AccountBalance.find_by(user: @user, asset: usd)
    assert_equal 1.to_d, cash.usd_price
    assert_equal 84_914.to_d, cash.usd_value
    assert_not_nil cash.priced_at
  end

  test 'cash in another currency is valued at the day\'s rate' do
    eur = create(:asset, :eur)
    @exchange.stubs(:get_balances).returns(Result::Success.new(eur.id => { free: 1_000.to_d, locked: 0 }))
    Utilities::Currency.stubs(:exchange_rate).with(from: 'EUR', to: 'USD').returns(Result::Success.new(1.1))
    MarketData.expects(:get_prices).never

    AccountBalance::Sync.new(@api_key).sync!

    assert_equal 1_100.to_d, AccountBalance.find_by(user: @user, asset: eur).usd_value
  end

  # A ticker is not a category: ProShares Ultra Semiconductors trades as USD, and it is a security
  # with a price like any other.
  test 'a security that merely spells a currency is still priced by the market' do
    usd_stock = create(:asset, symbol: 'USD', external_id: 'USD.US', category: 'Stock')
    @exchange.stubs(:get_balances).returns(Result::Success.new(usd_stock.id => { free: 2.to_d, locked: 0 }))
    MarketData.stubs(:get_prices).returns(Result::Success.new('USD.US' => 90.0))

    AccountBalance::Sync.new(@api_key).sync!

    assert_equal 180.to_d, AccountBalance.find_by(user: @user, asset: usd_stock).usd_value
  end

  test 'reuses stored price as stale fallback when fresh pricing fails' do
    # Seed a previous successful sync
    @exchange.stubs(:get_balances).returns(Result::Success.new(
                                             @btc.id => { free: 1.to_d, locked: 0 }
                                           ))
    MarketData.stubs(:get_prices).returns(Result::Success.new('bitcoin' => 50_000.0))
    AccountBalance::Sync.new(@api_key).sync!
    original_priced_at = AccountBalance.find_by(asset: @btc).priced_at

    # Next sync: qty changes, pricing fails
    travel 1.hour do
      @exchange.stubs(:get_balances).returns(Result::Success.new(
                                               @btc.id => { free: 2.to_d, locked: 0 }
                                             ))
      MarketData.stubs(:get_prices).returns(Result::Failure.new('CoinGecko API down'))

      result = AccountBalance::Sync.new(@api_key).sync!
      summary = result.data
      assert_equal 0, summary.priced_fresh
      assert_equal 1, summary.priced_stale
      assert_equal 'CoinGecko API down', summary.pricing_error
    end

    btc_bal = AccountBalance.find_by(asset: @btc)
    # usd_price retained, priced_at retained
    assert_equal 50_000.to_d, btc_bal.usd_price
    assert_equal original_priced_at.to_i, btc_bal.priced_at.to_i
    # usd_value recomputed against new quantity
    assert_equal (2.to_d * 50_000.to_d), btc_bal.usd_value
  end

  test 'leaves usd_price nil when no fresh price and no prior price' do
    @exchange.stubs(:get_balances).returns(Result::Success.new(
                                             @btc.id => { free: 1.to_d, locked: 0 }
                                           ))
    MarketData.stubs(:get_prices).returns(Result::Failure.new('down'))

    result = AccountBalance::Sync.new(@api_key).sync!
    summary = result.data
    assert_equal 1, summary.unpriced
    assert summary.pricing_fully_failed?

    btc_bal = AccountBalance.find_by(asset: @btc)
    assert_nil btc_bal.usd_price
    assert_nil btc_bal.priced_at
    assert_nil btc_bal.usd_value
  end

  test 'skips zero-balance assets' do
    @exchange.stubs(:get_balances).returns(Result::Success.new(
                                             @btc.id => { free: 0, locked: 0 },
                                             @eth.id => { free: 1.to_d, locked: 0 }
                                           ))
    MarketData.stubs(:get_prices).returns(Result::Success.new('ethereum' => 3_000.0))

    AccountBalance::Sync.new(@api_key).sync!

    assert_nil AccountBalance.find_by(user: @user, asset: @btc)
    assert_not_nil AccountBalance.find_by(user: @user, asset: @eth)
  end

  test 'deletes rows for assets that are gone from the exchange' do
    @exchange.stubs(:get_balances).returns(Result::Success.new(
                                             @btc.id => { free: 1.to_d, locked: 0 },
                                             @eth.id => { free: 5.to_d, locked: 0 }
                                           ))
    MarketData.stubs(:get_prices).returns(Result::Success.new('bitcoin' => 50_000.0, 'ethereum' => 3_000.0))
    AccountBalance::Sync.new(@api_key).sync!
    assert_equal 2, AccountBalance.count

    @exchange.stubs(:get_balances).returns(Result::Success.new(
                                             @btc.id => { free: 2.to_d, locked: 0 }
                                           ))
    MarketData.stubs(:get_prices).returns(Result::Success.new('bitcoin' => 60_000.0))
    AccountBalance::Sync.new(@api_key).sync!

    assert_equal 1, AccountBalance.count
  end

  test 'Alpaca stocks are priced from the exchange, not from MarketData' do
    alpaca = create(:alpaca_exchange)
    alpaca_key = create(:api_key, user: @user, exchange: alpaca)
    aapl = create(:asset, external_id: 'alpaca_AAPL', symbol: 'AAPL', name: 'Apple', category: 'Stock')
    alpaca.stubs(:set_client)
    alpaca.stubs(:get_balances).returns(Result::Success.new(
                                          aapl.id => { free: 10.to_d, locked: 0 }
                                        ))
    alpaca.expects(:get_tickers_prices).with(symbols: ['AAPL']).returns(Result::Success.new('AAPL' => 180.5))
    # Only the non-stock external_ids should reach MarketData. With nothing
    # left after the override, MarketData is not called at all.
    MarketData.expects(:get_prices).never

    result = AccountBalance::Sync.new(alpaca_key).sync!
    assert result.success?

    aapl_bal = AccountBalance.find_by(user: @user, asset: aapl)
    assert_equal 180.5.to_d, aapl_bal.usd_price
    assert_equal (10.to_d * 180.5.to_d), aapl_bal.usd_value
  end

  test 'returns failure when exchange get_balances fails' do
    @exchange.stubs(:get_balances).returns(Result::Failure.new('api error'))

    result = AccountBalance::Sync.new(@api_key).sync!

    assert result.failure?
    assert_equal 0, AccountBalance.count
  end
end
