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

  test 'priced? returns false and logs ticker id and error class when the exchange raises on zero price' do
    @ticker.stubs(:get_last_price).raises(RuntimeError.new("Wrong last price for #{@ticker.ticker}: 0.0"))
    Rails.logger.expects(:info)
         .with(regexp_matches(/Ticker#priced\? false for ticker=#{@ticker.id}.*RuntimeError/))
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

  # == unsupported price type ==

  test 'priced? raises ArgumentError for an unsupported price type' do
    assert_raises(ArgumentError) { @ticker.priced?(:bid) }
  end
end
