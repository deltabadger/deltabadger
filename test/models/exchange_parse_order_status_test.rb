require 'test_helper'

class ExchangeParseOrderStatusTest < ActiveSupport::TestCase
  # Characterization tests for parse_order_status (dedup slice 04, item 3),
  # pinning every status string each exchange currently recognizes before the
  # case statements are extracted into per-exchange ORDER_STATUS_MAP constants.
  # The 10 canonical exchanges raise on unknown statuses; alpaca, hyperliquid
  # and ibkr intentionally fall back to :unknown (owner decision 2026-06-12).
  # Gemini and KuCoin derive status from order_data flags and are out of scope.

  CANONICAL_MAPS = {
    Exchanges::Binance => {
      'PENDING_CANCEL' => :unknown,
      'NEW' => :open, 'PENDING_NEW' => :open, 'PARTIALLY_FILLED' => :open,
      'FILLED' => :closed,
      'CANCELED' => :cancelled, 'EXPIRED' => :cancelled, 'EXPIRED_IN_MATCH' => :cancelled,
      'REJECTED' => :failed
    },
    Exchanges::BinanceUs => {
      'PENDING_CANCEL' => :unknown,
      'NEW' => :open, 'PENDING_NEW' => :open, 'PARTIALLY_FILLED' => :open,
      'FILLED' => :closed,
      'CANCELED' => :cancelled, 'EXPIRED' => :cancelled, 'EXPIRED_IN_MATCH' => :cancelled,
      'REJECTED' => :failed
    },
    Exchanges::Bingx => {
      'NEW' => :open, 'PARTIALLY_FILLED' => :open, 'PENDING' => :open,
      'FILLED' => :closed,
      'CANCELED' => :cancelled, 'EXPIRED' => :cancelled,
      'REJECTED' => :failed, 'FAILED' => :failed
    },
    Exchanges::Bitget => {
      'init' => :unknown, 'new' => :unknown,
      'partially_filled' => :open, 'live' => :open,
      'filled' => :closed,
      'cancelled' => :cancelled
    },
    Exchanges::Bitmart => {
      'new' => :open, 'partially_filled' => :open,
      'filled' => :closed,
      'canceled' => :cancelled, 'expired' => :cancelled, 'partially_canceled' => :cancelled,
      'rejected' => :failed, 'failed' => :failed
    },
    Exchanges::Bitrue => {
      'NEW' => :open, 'PARTIALLY_FILLED' => :open,
      'FILLED' => :closed,
      'CANCELED' => :cancelled, 'EXPIRED' => :cancelled,
      'REJECTED' => :failed
    },
    Exchanges::Bitvavo => {
      'new' => :open, 'partiallyFilled' => :open,
      'filled' => :closed,
      'canceled' => :cancelled, 'cancelled' => :cancelled, 'expired' => :cancelled,
      'rejected' => :failed
    },
    Exchanges::Bybit => {
      'Created' => :unknown, 'Untriggered' => :unknown,
      'New' => :open, 'PartiallyFilled' => :open, 'PartiallyFilledCanceled' => :open,
      'Filled' => :closed,
      'Cancelled' => :cancelled, 'Expired' => :cancelled, 'Deactivated' => :cancelled,
      'Rejected' => :failed
    },
    Exchanges::Coinbase => {
      'PENDING' => :unknown, 'UNKNOWN_ORDER_STATUS' => :unknown,
      'OPEN' => :open,
      'FILLED' => :closed,
      'CANCELLED' => :cancelled, 'EXPIRED' => :cancelled,
      'FAILED' => :failed
    },
    Exchanges::Kraken => {
      'pending' => :unknown,
      'open' => :open,
      'closed' => :closed,
      'canceled' => :cancelled, 'expired' => :cancelled
    },
    Exchanges::Mexc => {
      'NEW' => :open, 'PARTIALLY_FILLED' => :open,
      'FILLED' => :closed,
      'CANCELED' => :cancelled, 'EXPIRED' => :cancelled,
      'REJECTED' => :failed
    }
  }.freeze

  CANONICAL_MAPS.each do |klass, map|
    test "#{klass.name.demodulize} maps every known status as before and raises on unknown" do
      exchange = klass.new(name: klass.name.demodulize)

      map.each do |status, expected|
        assert_equal expected, exchange.send(:parse_order_status, status), "status #{status.inspect}"
      end

      error = assert_raises(RuntimeError) { exchange.send(:parse_order_status, 'BOGUS_STATUS') }
      assert_equal "Unknown #{exchange.name} order status: BOGUS_STATUS", error.message
    end
  end

  FALLBACK_MAPS = {
    Exchanges::Alpaca => {
      'new' => :open, 'accepted' => :open, 'pending_new' => :open,
      'filled' => :closed,
      'canceled' => :cancelled, 'expired' => :cancelled, 'replaced' => :cancelled,
      'rejected' => :failed
    },
    # Corrected (2026-06-17): a margin-cancel IS a cancel; a triggered order has fired and
    # become a live resting order, so it is open. Matching is suffix-aware (/cancel/i, /reject/i)
    # so the full Hyperliquid cancel family (scheduledCancel, selfTradeCanceled, …) maps correctly.
    Exchanges::Hyperliquid => {
      'open' => :open, 'triggered' => :open,
      'filled' => :closed,
      'canceled' => :cancelled, 'marginCanceled' => :cancelled, 'scheduledCancel' => :cancelled,
      'selfTradeCanceled' => :cancelled, 'rejected' => :cancelled,
      'unknownOid' => :unknown
    },
    Exchanges::Ibkr => {
      'Filled' => :closed,
      'Cancelled' => :cancelled, 'PendingCancel' => :cancelled, 'Inactive' => :cancelled,
      'Submitted' => :open, 'PreSubmitted' => :open, 'PendingSubmit' => :open,
      'Rejected' => :failed
    }
  }.freeze

  FALLBACK_MAPS.each do |klass, map|
    test "#{klass.name.demodulize} maps every known status as before and falls back to :unknown" do
      exchange = klass.new(name: klass.name.demodulize)

      map.each do |status, expected|
        assert_equal expected, exchange.send(:parse_order_status, status), "status #{status.inspect}"
      end

      assert_equal :unknown, exchange.send(:parse_order_status, 'BOGUS_STATUS')
    end
  end

  test 'IBKR coerces non-string statuses before matching' do
    exchange = Exchanges::Ibkr.new(name: 'IBKR')

    assert_equal :unknown, exchange.send(:parse_order_status, nil)
    assert_equal :closed, exchange.send(:parse_order_status, :Filled)
  end
end
