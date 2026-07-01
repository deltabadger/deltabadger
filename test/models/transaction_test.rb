require 'test_helper'

class TransactionTest < ActiveSupport::TestCase
  # == waiting scope ==
  # "waiting" = a real order accepted by the exchange whose execution is not yet
  # confirmed: submitted rows with external_status open OR unknown. unknown rows
  # exist after Finding 1 (immediate persist) when the confirmation fetch has not
  # yet succeeded — they must still be treated as in-flight everywhere.

  test 'waiting scope returns submitted orders that are open or unknown' do
    bot = create(:dca_single_asset)
    open_txn = create(:transaction, bot: bot, status: :submitted, external_status: :open, external_id: 'o1')
    unknown_txn = create(:transaction, bot: bot, status: :submitted, external_status: :unknown, external_id: 'u1')
    closed_txn = create(:transaction, bot: bot, status: :submitted, external_status: :closed, external_id: 'c1')
    cancelled_txn = create(:transaction, bot: bot, status: :submitted, external_status: :cancelled, external_id: 'x1')

    waiting = bot.transactions.waiting

    assert_includes waiting, open_txn
    assert_includes waiting, unknown_txn
    assert_not_includes waiting, closed_txn
    assert_not_includes waiting, cancelled_txn
  end

  test 'waiting scope excludes failed and skipped rows even if external_status is set' do
    bot = create(:dca_single_asset)
    create(:transaction, bot: bot, status: :failed, external_status: :unknown, external_id: 'f1')
    create(:transaction, bot: bot, status: :skipped, external_status: :open, external_id: 's1')

    assert_empty bot.transactions.waiting
  end

  # == :abandoned external_status ==
  # An order Kraken (or any exchange) no longer tracks. Treated like cancelled
  # for filter/UI purposes, but emitted via a distinct lifecycle path so the
  # activity feed can explain the difference.

  test 'abandoned is a valid external_status enum value' do
    txn = build(:transaction, status: :submitted, external_status: :abandoned, external_id: 'a1')
    assert txn.abandoned?
    assert_equal 'abandoned', txn.external_status
  end

  test 'waiting scope excludes abandoned rows' do
    bot = create(:dca_single_asset)
    abandoned_txn = create(:transaction, bot: bot, status: :submitted, external_status: :abandoned, external_id: 'a1')

    assert_not_includes bot.transactions.waiting, abandoned_txn
  end

  # == update_with_order_data: base/quote preservation (post-incident 2026-05-28) ==
  # Before the fix, `base: order_data[:ticker]&.base_asset&.symbol || base` overwrote the
  # historical transaction's base with the CURRENT local asset symbol on every order poll.
  # If the local asset row's symbol mutated (e.g. via the stocks sync incident), the poll
  # propagated that mutation retroactively into transaction history. Bot 5 tx 168/169 went
  # from base=IBIT to base=LDRC this way, with no Alpaca-side change. Fix: only set
  # base/quote on the first insert; preserve them on subsequent updates.

  test 'update_with_order_data preserves existing base when local ticker symbol changes' do
    bot = create(:dca_single_asset)
    btc = Asset.where(external_id: 'bitcoin').first || create(:asset, :bitcoin)
    usd = Asset.where(external_id: 'usd').first || create(:asset, :usd)
    exchange = create(:kraken_exchange)
    new_ticker = create(:ticker, exchange: exchange, base_asset: btc, quote_asset: usd, base: 'XBT', quote: 'USD', ticker: 'XBTUSD')

    txn = create(:transaction, bot: bot, exchange: exchange, status: :submitted, base: 'BTC', quote: 'USD',
                               external_id: 'tx_preserve', side: :buy)

    txn.update_with_order_data(
      status: :open, price: 1000, amount: 0.01, quote_amount: 10,
      ticker: new_ticker, side: :buy, order_type: :market_order,
      amount_exec: 0.01, quote_amount_exec: 10
    )

    txn.reload
    assert_equal 'BTC', txn.base, 'historical base must not be rewritten by a mutated local ticker'
    assert_equal 'USD', txn.quote, 'historical quote must not be rewritten by a mutated local ticker'
  end

  test 'update_with_order_data populates base/quote on first set when blank' do
    bot = create(:dca_single_asset)
    btc = Asset.where(external_id: 'bitcoin').first || create(:asset, :bitcoin)
    usd = Asset.where(external_id: 'usd').first || create(:asset, :usd)
    exchange = create(:kraken_exchange)
    ticker = create(:ticker, exchange: exchange, base_asset: btc, quote_asset: usd, base: 'BTC', quote: 'USD', ticker: 'BTCUSD')

    txn = build(:transaction, bot: bot, exchange: exchange, status: :submitted, base: nil, quote: nil,
                              external_id: 'tx_first_set', side: :buy)
    txn.save!(validate: false)

    txn.update_with_order_data(
      status: :open, price: 1000, amount: 0.01, quote_amount: 10,
      ticker: ticker, side: :buy, order_type: :market_order,
      amount_exec: 0.01, quote_amount_exec: 10
    )

    txn.reload
    assert_equal 'BTC', txn.base, 'blank base must be populated from the ticker on first set'
    assert_equal 'USD', txn.quote, 'blank quote must be populated from the ticker on first set'
  end

  test 'cancelled_or_abandoned scope returns both cancelled and abandoned rows but no others' do
    bot = create(:dca_single_asset)
    cancelled_txn = create(:transaction, bot: bot, status: :submitted, external_status: :cancelled, external_id: 'c1')
    abandoned_txn = create(:transaction, bot: bot, status: :submitted, external_status: :abandoned, external_id: 'a1')
    open_txn = create(:transaction, bot: bot, status: :submitted, external_status: :open, external_id: 'o1')
    closed_txn = create(:transaction, bot: bot, status: :submitted, external_status: :closed, external_id: 'cl1')

    rows = bot.transactions.cancelled_or_abandoned

    assert_includes rows, cancelled_txn
    assert_includes rows, abandoned_txn
    assert_not_includes rows, open_txn
    assert_not_includes rows, closed_txn
  end

  # == metrics cache invalidation ==
  # Metrics count an order only once it is closed; a close with no quote-exec change (an exchange that
  # closes an order with nil exec, or a legacy fill) must still invalidate the 30-day metrics cache.

  test 'metrics_relevant_change? is true on the transition to closed even when exec amounts stay nil' do
    bot = create(:dca_single_asset, :started)
    order = create(:transaction, bot: bot, side: :buy, status: :submitted, external_status: :open,
                                 external_id: 'o1', amount: 1, price: 100, quote_amount: 100,
                                 amount_exec: nil, quote_amount_exec: nil)

    order.update!(external_status: :closed) # closed, but exec amounts still nil

    assert_not order.send(:custom_quote_amount_exec_changed?), 'quote exec did not change'
    assert order.send(:metrics_relevant_change?), 'the close must still trigger a metrics refresh'
  end

  test 'metrics_relevant_change? is false when an unrelated field changes on an already-closed order' do
    bot = create(:dca_single_asset, :started)
    order = create(:transaction, bot: bot, side: :buy, status: :submitted, external_status: :closed,
                                 external_id: 'c1', amount: 1, price: 100, quote_amount: 100,
                                 amount_exec: 1, quote_amount_exec: 100)

    order.update!(price: 101)

    assert_not order.send(:metrics_relevant_change?)
  end

  # The base cap counts a sell once closed (fallback to requested amount if amount_exec is nil), so a
  # close with no amount_exec change must still re-evaluate the cap.
  test 'base_cap_relevant_change? is true on the transition to closed even when amount_exec stays nil' do
    bot = create(:dca_single_asset, :started)
    order = create(:transaction, bot: bot, side: :sell, status: :submitted, external_status: :open,
                                 external_id: 'o1', amount: 1, price: 100, quote_amount: 100,
                                 amount_exec: nil, quote_amount_exec: nil)

    order.update!(external_status: :closed)

    assert_not order.send(:custom_amount_exec_changed?), 'amount_exec did not change'
    assert order.send(:base_cap_relevant_change?), 'the close must still re-evaluate the base cap'
  end
end
