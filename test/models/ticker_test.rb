require 'test_helper'

class TickerTest < ActiveSupport::TestCase
  include ExchangeMockHelpers

  setup do
    @ticker = create(:ticker, :btc_usd, exchange: create(:kraken_exchange))
  end

  # == #priced? : :last (default) ==

  test 'priced? returns true when last price is positive' do
    stub_ticker_last_price(@ticker, price: BigDecimal('100'))
    assert @ticker.priced?(:last)
    assert @ticker.priced? # defaults to :last
  end

  test 'priced? returns false when last price lookup fails' do
    @ticker.stubs(:get_last_price).returns(Result::Failure.new('boom'))
    assert_not @ticker.priced?(:last)
  end

  test 'priced? returns false when last price is zero' do
    stub_ticker_last_price(@ticker, price: BigDecimal('0'))
    assert_not @ticker.priced?(:last)
  end

  test 'priced? returns false and logs at debug (not info/error) when the exchange raises on zero price' do
    @ticker.stubs(:get_last_price).raises(RuntimeError.new("Wrong last price for #{@ticker.ticker}: 0.0"))
    # Normal flow, not an error — must stay below the log exception scanner's threshold.
    Rails.logger.expects(:info).never
    Rails.logger.expects(:debug)
         .with(regexp_matches(/Ticker#priced\? false for ticker=#{@ticker.id}/))
         .at_least_once
    assert_not @ticker.priced?(:last)
  end

  # == #priced? : :ask ==

  test 'priced? returns true when ask price is positive' do
    stub_ticker_ask_price(@ticker, price: BigDecimal('100'))
    assert @ticker.priced?(:ask)
  end

  test 'priced? returns false when ask price is zero' do
    stub_ticker_ask_price(@ticker, price: BigDecimal('0'))
    assert_not @ticker.priced?(:ask)
  end

  test 'priced? returns false when ask price lookup fails' do
    @ticker.stubs(:get_ask_price).returns(Result::Failure.new('boom'))
    assert_not @ticker.priced?(:ask)
  end

  # == dispatch ==

  test 'priced?(:ask) probes ask price, not last price' do
    @ticker.expects(:get_ask_price).returns(Result::Success.new(BigDecimal('100')))
    @ticker.expects(:get_last_price).never
    assert @ticker.priced?(:ask)
  end

  test 'priced? default probes last price, not ask price' do
    @ticker.expects(:get_last_price).returns(Result::Success.new(BigDecimal('100')))
    @ticker.expects(:get_ask_price).never
    assert @ticker.priced?
  end

  test 'priced? returns true when bid price is positive' do
    stub_ticker_bid_price(@ticker, price: BigDecimal('100'))
    assert @ticker.priced?(:bid)
  end

  test 'priced? returns false when bid price is zero' do
    stub_ticker_bid_price(@ticker, price: BigDecimal('0'))
    assert_not @ticker.priced?(:bid)
  end

  # == unsupported price type ==

  test 'priced? raises ArgumentError for an unsupported price type' do
    assert_raises(ArgumentError) { @ticker.priced?(:mid) }
  end

  # == #tradeable? ==

  test 'tradeable?(:buy) is true when trading_enabled and the ask is positive' do
    @ticker.trading_enabled = true
    stub_ticker_ask_price(@ticker, price: BigDecimal('100'))
    assert @ticker.tradeable?(:buy)
  end

  test 'tradeable?(:buy) probes the ask price' do
    @ticker.trading_enabled = true
    @ticker.expects(:get_ask_price).returns(Result::Success.new(BigDecimal('100')))
    @ticker.expects(:get_bid_price).never
    assert @ticker.tradeable?(:buy)
  end

  test 'tradeable?(:sell) probes the bid price' do
    @ticker.trading_enabled = true
    @ticker.expects(:get_bid_price).returns(Result::Success.new(BigDecimal('100')))
    @ticker.expects(:get_ask_price).never
    assert @ticker.tradeable?(:sell)
  end

  test 'tradeable? short-circuits to false (no price call) when trading is disabled' do
    @ticker.trading_enabled = false
    @ticker.expects(:get_ask_price).never
    @ticker.expects(:get_bid_price).never
    assert_not @ticker.tradeable?(:buy)
  end

  test 'tradeable? is false when trading_enabled but unpriced' do
    @ticker.trading_enabled = true
    @ticker.stubs(:get_ask_price).returns(Result::Failure.new('boom'))
    assert_not @ticker.tradeable?(:buy)
  end

  test 'tradeable? raises ArgumentError for an unsupported side' do
    assert_raises(ArgumentError) { @ticker.tradeable?(:hodl) }
  end

  # == #priced? : transient network passthrough ==

  test 'priced? re-raises Client::TransientNetworkError instead of degrading to false' do
    @ticker.stubs(:get_last_price).raises(Client::TransientNetworkError, 'Net::OpenTimeout: TCP open timed out')
    Rails.logger.expects(:debug).never

    assert_raises(Client::TransientNetworkError) { @ticker.priced?(:last) }
  end

  test 'priced? still swallows other StandardErrors and logs at debug' do
    @ticker.stubs(:get_last_price).raises(RuntimeError, 'boom')
    Rails.logger.expects(:debug)
         .with(regexp_matches(/Ticker#priced\? false for ticker=#{@ticker.id}/))
         .at_least_once

    assert_not @ticker.priced?(:last)
  end

  # == #adjusted_price : delegates to the exchange; default = fixed decimal places ==

  test 'adjusted_price floors to price_decimals for a non-Hyperliquid exchange' do
    @ticker.update!(price_decimals: 2)
    assert_equal BigDecimal('100.12'), @ticker.adjusted_price(price: BigDecimal('100.12999'))
  end

  test 'adjusted_price honors the :round method for a non-Hyperliquid exchange' do
    @ticker.update!(price_decimals: 2)
    assert_equal BigDecimal('100.13'), @ticker.adjusted_price(price: BigDecimal('100.12999'), method: :round)
  end
end
