require 'test_helper'

# DRY_RUN mode. The whole test suite runs in it (config/environments/test.rb), and it is what a
# developer runs locally instead of trading real money.
#
# Balances used to be rand(100..10_000) per call, which made the local app untestable in a way that
# looked like product bugs: a liquidation sizes at min(ledger, free balance), so selling 15M THETA
# placed an order for 950 of it, then 9,849, then 1,009 — each click a different sliver, the coin
# never leaving the portfolio, and nothing anywhere admitting why.
class Exchange::DryableTest < ActiveSupport::TestCase
  include ExchangeMockHelpers

  def setup
    @bot = create(:dca_index, user: create(:user), with_api_key: true)
    @exchange = @bot.exchange
    @btc = create(:asset, symbol: 'BTC', name: 'Bitcoin', external_id: 'bitcoin')
    @ticker = create(:ticker, exchange: @exchange, base_asset: @btc, quote_asset: @bot.quote_asset)
  end

  test 'the balance of a held asset is what the bot actually holds' do
    buy!('BTC', amount: 3)

    assert_in_delta 3, free('BTC'), 0.00000001
  end

  test 'the balance is the net of buys and sells, not the gross bought' do
    buy!('BTC', amount: 3)
    sell!('BTC', amount: 1.25)

    assert_in_delta 1.75, free('BTC'), 0.00000001
  end

  test 'the same asset reads the same twice' do
    # The old version rolled a fresh number per call, so two reads inside one sale disagreed with
    # each other and with the table the user was looking at.
    buy!('BTC', amount: 3)

    assert_equal free('BTC'), free('BTC')
  end

  test 'only executed amounts count' do
    # A failed or skipped order bought nothing, and a sale sized against it would be sizing against
    # an order the exchange rejected.
    buy!('BTC', amount: 3)
    buy!('BTC', amount: 99, status: :failed, amount_exec: 0)
    buy!('BTC', amount: 77, status: :skipped, amount_exec: 0)

    assert_in_delta 3, free('BTC'), 0.00000001
  end

  test 'another exchange holdings do not count toward this one' do
    other_bot = create(:dca_index, user: create(:user), exchange: create(:binance_exchange),
                                   quote_asset: @bot.quote_asset)
    create(:ticker, exchange: other_bot.exchange, base_asset: @btc, quote_asset: other_bot.quote_asset)
    buy!('BTC', amount: 3)
    order!('BTC', :buy, 50, :submitted, 50, other_bot)

    assert_in_delta 3, free('BTC'), 0.00000001
  end

  test 'a closed order with no recorded fill counts what it asked for' do
    # COALESCE(amount_exec, amount) on a closed row, the same fallback Bot#total_amount,
    # Bot::BaseAmountLimitable and the metrics all use. Reading it as zero instead would put the
    # balance BELOW the ledger the page shows — and min(ledger, balance) would then be back to
    # selling a sliver of a position the user can see in full.
    buy!('BTC', amount: 2, amount_exec: nil)

    assert_in_delta 2, free('BTC'), 0.00000001
  end

  test 'an order still working counts for nothing until it closes' do
    buy!('BTC', amount: 2)
    buy!('BTC', amount: 40, external_status: :open)

    assert_in_delta 2, free('BTC'), 0.00000001
  end

  test 'two assets wearing the same symbol both report the holding' do
    # Symbols are not unique — a Hyperliquid RWA and the Alpaca stock it tracks share one, and the
    # FIGI work split them into separate rows ON PURPOSE. A transaction records only the symbol, so
    # neither can claim the ledger alone. Keying a hash by symbol dropped one of them, and the
    # survivor took the whole balance while the other fell through to the untraded figure.
    twin = create(:asset, symbol: 'BTC', name: 'Bitcoin token', external_id: 'bitcoin-token')
    create(:exchange_asset, exchange: @exchange, asset: twin)
    buy!('BTC', amount: 2)

    assert_in_delta 2, @exchange.get_balance(asset_id: @btc.id).data[:free].to_d, 0.00000001
    assert_in_delta 2, @exchange.get_balance(asset_id: twin.id).data[:free].to_d, 0.00000001
  end

  test 'an asset nothing on the venue prices in reads zero, not a fortune' do
    # The fallback exists to fund BUYING, so it belongs to assets something actually quotes in.
    # Handing it to any asset we failed to match a ledger row for would size a sale against a
    # million units that do not exist — the oversizing mirror of the bug this all started with.
    orphan = create(:asset, symbol: 'ORPH', name: 'Orphan', external_id: 'orphan')
    create(:exchange_asset, exchange: @exchange, asset: orphan)

    assert_in_delta 0, free('ORPH'), 0.00000001
  end

  test 'a whole-wallet read does not invent a balance for every listed asset' do
    # AccountBalance::Sync and the exchanges API ask for every asset at once, and persist what they
    # get. Funding all of them would put a million units of each into the tracker — on a stock
    # exchange that is thousands of assets and a meaningless portfolio total.
    buy!('BTC', amount: 2)

    balances = @exchange.get_balances.data

    assert_in_delta 2, balances[@btc.id][:free].to_d, 0.00000001
    assert(balances.except(@btc.id).values.all? { |b| b[:free].to_d.zero? })
  end

  test 'an asset the bots never held is funded, so buying still works locally' do
    # The quote currency is never bought, only spent — derived from the ledger it would be negative,
    # and every dry buy would fail for insufficient funds.
    assert_operator free(@bot.quote_asset.symbol), :>, 1_000
  end

  test 'a position sold down to nothing does not read as a fortune' do
    # The fallback is for assets outside the ledger entirely. A holding that has been closed is IN
    # the ledger at zero, and reporting a windfall there would let a sale be sized against coins
    # that are gone.
    buy!('BTC', amount: 3)
    sell!('BTC', amount: 3)

    assert_in_delta 0, free('BTC'), 0.00000001
  end

  test 'a balance is never negative' do
    sell!('BTC', amount: 5)

    assert_operator free('BTC'), :>=, 0
  end

  private

  def free(symbol)
    asset = Asset.find_by(symbol: symbol)
    @exchange.get_balance(asset_id: asset.id).data[:free].to_d
  end

  def buy!(base, amount:, status: :submitted, amount_exec: :same, external_status: :closed)
    order!(base, :buy, amount, status, amount_exec == :same ? amount : amount_exec, nil,
           external_status: external_status)
  end

  def sell!(base, amount:, status: :submitted, amount_exec: :same, external_status: :closed)
    order!(base, :sell, amount, status, amount_exec == :same ? amount : amount_exec, nil,
           external_status: external_status)
  end

  def order!(base, side, amount, status = :submitted, amount_exec = nil, bot = nil, external_status: :closed)
    bot ||= @bot
    create(:transaction, bot: bot, exchange: bot.exchange, side: side, status: status,
                         external_status: external_status,
                         base: base, quote: bot.quote_asset.symbol, price: 100,
                         amount: amount, amount_exec: amount_exec,
                         quote_amount: amount * 100, quote_amount_exec: (amount_exec || amount) * 100,
                         external_id: "t-#{SecureRandom.hex(6)}")
  end
end
