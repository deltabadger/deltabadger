require 'test_helper'

# A split multiplies a share count with nothing bought or sold. The bot's own ledger records orders
# only, so unless the walk folds the broker's restatement in, every number downstream of it — the
# headline, the chart, the weights an order is sized against, the amount a liquidation offers — is
# wrong by the factor, permanently.
class Bot::Composition::RestatedHoldingsTest < ActiveSupport::TestCase
  setup do
    @user = create(:user)
    @exchange = create(:alpaca_exchange)
    @api_key = create(:api_key, user: @user, exchange: @exchange)
    @usd = Asset.find_by(symbol: 'USD') || create(:asset, :usd)
    @klac = create(:asset, external_id: 'klac', symbol: 'KLAC')
    @aapl = create(:asset, external_id: 'aapl', symbol: 'AAPL')
    @bot = create(:dca_multi_asset, user: @user, exchange: @exchange, with_api_key: false,
                                    base_assets: [@klac, @aapl], quote_asset: @usd)
  end

  def buy(symbol, amount:, price:, at:)
    create(:transaction, bot: @bot, exchange: @exchange, base: symbol, quote: 'USD', side: :buy,
                         amount: amount, amount_exec: amount, price: price,
                         quote_amount: amount * price, quote_amount_exec: amount * price,
                         created_at: at)
  end

  def split(symbol: 'KLAC', ratio: '10:1', at: 5.days.ago, **overrides)
    create(:account_transaction, { user: @user, api_key: @api_key, exchange: @exchange,
                                   entry_type: :adjustment, base_currency: symbol, base_amount: 90,
                                   quote_currency: nil, quote_amount: nil, transacted_at: at,
                                   raw_data: { 'corporate_action' => 'split',
                                               'split_ratio' => ratio }.compact }.merge(overrides))
  end

  def held(symbol, bot = @bot)
    bot.metrics(force: true).dig(:asset_breakdown, symbol, :amount).to_d
  end

  def invested(symbol, bot = @bot)
    bot.metrics(force: true).dig(:asset_breakdown, symbol, :quote_invested).to_d
  end

  # ---- the case every bot in the fleet is in ----

  test 'a bot with no restatement is untouched' do
    buy('KLAC', amount: 2, price: 100, at: 8.days.ago)
    buy('AAPL', amount: 1, price: 50, at: 7.days.ago)
    before = @bot.metrics(force: true)

    after = @bot.metrics(force: true)

    assert_equal before, after
    assert_equal 2.to_d, held('KLAC')
  end

  # ---- the fold ----

  test 'a split between two buys multiplies the position and leaves the money alone' do
    buy('KLAC', amount: 2, price: 1000, at: 8.days.ago)
    split(at: 5.days.ago)
    buy('KLAC', amount: 3, price: 100, at: 2.days.ago)

    assert_equal (2 * 10) + 3, held('KLAC')
    assert_equal 2000 + 300, invested('KLAC'), 'a split moves no money'
  end

  test 'a split after the last order still restates the position' do
    buy('KLAC', amount: 2, price: 1000, at: 8.days.ago)
    split(at: 2.days.ago)

    assert_equal 20.to_d, held('KLAC')
  end

  test 'a split before the first order restates nothing' do
    split(at: 9.days.ago)
    buy('KLAC', amount: 2, price: 100, at: 8.days.ago)

    assert_equal 2.to_d, held('KLAC')
  end

  test 'a split of another symbol leaves this one alone' do
    buy('KLAC', amount: 2, price: 100, at: 8.days.ago)
    buy('AAPL', amount: 4, price: 50, at: 8.days.ago)
    split(symbol: 'AAPL', at: 5.days.ago)

    assert_equal 2.to_d, held('KLAC')
    assert_equal 40.to_d, held('AAPL')
  end

  test 'two restatements compound' do
    buy('KLAC', amount: 2, price: 1000, at: 9.days.ago)
    split(at: 6.days.ago, ratio: '2:1', tx_id: 'first')
    split(at: 3.days.ago, ratio: '5:1', tx_id: 'second')

    assert_equal 20.to_d, held('KLAC')
  end

  test 'a reverse split divides' do
    buy('KLAC', amount: 80, price: 1, at: 8.days.ago)
    split(at: 5.days.ago, ratio: '1:8')

    assert_equal 10.to_d, held('KLAC')
  end

  test 'a split at an order timestamp is applied before that order' do
    at = 5.days.ago.change(usec: 0)
    buy('KLAC', amount: 2, price: 1000, at: 8.days.ago)
    split(at: at)
    buy('KLAC', amount: 1, price: 100, at: at)

    assert_equal 21.to_d, held('KLAC'), 'the order acts on the position the split left'
  end

  # ---- the value it is all read through ----

  test 'the portfolio value does not jump when a split arrives with no trade after it' do
    buy('KLAC', amount: 2, price: 1000, at: 8.days.ago)
    before = @bot.metrics(force: true)[:total_amount_value_in_quote]

    split(at: 5.days.ago)

    assert_equal before, @bot.metrics(force: true)[:total_amount_value_in_quote],
                 'the count went up ten-fold and the last price it traded at went down ten-fold'
  end

  test 'the split takes a chart point of its own, and the curve does not step' do
    buy('KLAC', amount: 2, price: 1000, at: 8.days.ago)
    points_before = @bot.metrics(force: true)[:chart][:labels].size

    split(at: 5.days.ago)
    chart = @bot.metrics(force: true)[:chart]

    assert_equal points_before + 1, chart[:labels].size
    assert_equal chart[:series][0][-2], chart[:series][0][-1], 'value is continuous across it'
    assert_equal 20.to_d, chart[:extra_series].last['KLAC']
  end

  # ---- sourcing ----

  test 'an adjustment with no provenance restates nothing' do
    buy('KLAC', amount: 2, price: 100, at: 8.days.ago)
    split(at: 5.days.ago, raw_data: { 'activity_type' => 'JNLC' })

    assert_equal 2.to_d, held('KLAC')
  end

  test 'a split with no readable factor restates nothing' do
    buy('KLAC', amount: 2, price: 100, at: 8.days.ago)
    split(at: 5.days.ago, ratio: nil)

    assert_equal 2.to_d, held('KLAC')
  end

  test 'a symbol with no position is not conjured into the breakdown by its own split' do
    # The order never executed, so there is no ledger entry — but the symbol is still one the bot
    # traded, so its split is still loaded.
    create(:transaction, bot: @bot, exchange: @exchange, base: 'AAPL', quote: 'USD', side: :buy,
                         status: :skipped, amount: nil, amount_exec: nil, price: nil,
                         created_at: 8.days.ago)
    buy('KLAC', amount: 2, price: 100, at: 8.days.ago)
    split(symbol: 'AAPL', at: 5.days.ago)

    assert_equal ['KLAC'], @bot.metrics(force: true)[:asset_breakdown].keys
  end
end
