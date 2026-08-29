require 'test_helper'

class ExchangeTest < ActiveSupport::TestCase
  # §8 stock-venue routing: a first-class notion of "which exchanges trade stocks"
  # so Alpaca is no longer the hardcoded sole stock venue.
  test 'stock_venue? is true for the stock brokers and false for crypto exchanges' do
    assert_predicate create(:alpaca_exchange), :stock_venue?
    assert_predicate create(:ibkr_exchange), :stock_venue?
    refute_predicate create(:binance_exchange), :stock_venue?
  end

  test 'stock_venues scope returns only the stock brokers' do
    alpaca = create(:alpaca_exchange)
    ibkr = create(:ibkr_exchange)
    create(:binance_exchange)

    assert_equal [alpaca.id, ibkr.id].sort, Exchange.stock_venues.pluck(:id).sort
  end
  # == 401 classification ==
  #
  # Two different questions, deliberately answered differently. SAYING the venue rejected us is
  # cheap and self-correcting, so a bare 401 is enough — and for Coinbase it is the ONLY signal,
  # since it names no rejection string. CONDEMNING the stored key is not: see
  # ApiKeyFailureHandlingTest.
  test 'a 401 marks a credential failure even on a venue that names no rejection string' do
    coinbase = create(:coinbase_exchange)

    assert_empty Array(coinbase.known_errors[:invalid_key]), 'fixture: the venue names nothing'
    assert coinbase.invalid_key_error?(['Unauthorized'], status: 401)
    assert_not coinbase.condemning_invalid_key_error?(['Unauthorized'])
  end

  test 'a 401 does not mark a credential failure on IBKR, whose 401 is ambiguous' do
    ibkr = create(:ibkr_exchange)

    assert_predicate ibkr, :ambiguous_unauthorized?
    assert_not ibkr.invalid_key_error?(['not authenticated'], status: 401)
  end

  test 'a non-401 status is classified by the venue words alone' do
    kraken = create(:kraken_exchange)

    assert_not kraken.invalid_key_error?(['EGeneral:Internal error'], status: 500)
    assert kraken.invalid_key_error?(['EAPI:Invalid key'])
  end

  test 'raise_on_invalid_key! names the exchange and passes a success through' do
    coinbase = create(:coinbase_exchange)

    error = assert_raises(RuntimeError) do
      coinbase.raise_on_invalid_key!(Result::Failure.new('Unauthorized', data: { status: 401 }))
    end
    assert_match 'Coinbase rejected the API key', error.message

    assert_nil coinbase.raise_on_invalid_key!(Result::Success.new({}))
  end

  # Corporate actions only exist on stock venues, so for a crypto exchange the indicator series
  # IS the as-traded series — no signature and no behaviour of its own.
  test 'get_indicator_candles falls through to get_candles where nothing restates history' do
    binance = create(:binance_exchange)
    ticker = create(:ticker, exchange: binance)
    candles = [[Time.now.utc, 1.to_d, 2.to_d, 0.5.to_d, 1.5.to_d, 10.to_d]]
    binance.expects(:get_candles).with(ticker: ticker, start_at: anything, timeframe: 1.day)
           .returns(Result::Success.new(candles))

    result = binance.get_indicator_candles(ticker: ticker, start_at: 1.day.ago, timeframe: 1.day)

    assert_equal candles, result.data
    assert_not binance.restated_candles?(ticker)
  end
end
