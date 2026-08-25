require 'test_helper'

# The page's figures, from one place.
#
# They used to be six independent calculations over two different sources of truth — the ledger
# (transactions, FIFO lots) and the balances (what a venue reports) — with nothing tying them. So
# nothing could notice when they contradicted each other, and the page stated all six as fact.
#
# The rule here: a figure is stated when it can be VOUCHED FOR, and the ledger says plainly when it
# cannot. Two things it can always check without asking anyone:
#
#   * a running quantity that goes NEGATIVE — you cannot hold minus six litecoin, so the history is
#     provably incomplete, and FIFO's floor-at-zero would otherwise launder it into a confident
#     positive;
#   * its own quantity against the one the venue reports — both numbers are in hand for every
#     connected exchange, and nothing ever compared them.
class Tracker::FiguresTest < ActiveSupport::TestCase
  setup do
    Tax::EcbFxRates.stubs(:ensure_loaded!)
    Rails.stubs(:cache).returns(ActiveSupport::Cache::MemoryStore.new)
    @user = create(:user)
    @binance = create(:binance_exchange)
    @key = create(:api_key, user: @user, exchange: @binance)
    @btc = create(:asset, :bitcoin)
    @day = ->(n) { Time.utc(2026, 1, n, 12) }
  end

  def tx(type, day:, **attrs)
    defaults = { api_key: @key, exchange: @binance, entry_type: type, transacted_at: @day.call(day) }
    defaults.merge!(quote_currency: nil, quote_amount: nil) if %i[deposit withdrawal].include?(type)
    create(:account_transaction, **defaults, **attrs)
  end

  def balance(asset, quantity, value)
    AccountBalance.create!(user: @user, exchange: @binance, asset: asset, free: quantity, locked: 0,
                           usd_price: quantity.zero? ? 0 : value / quantity, usd_value: value,
                           synced_at: Time.current, priced_at: Time.current)
  end

  def figures
    Tracker::Figures.for(@user,
                         ledger: Tracker::Ledger.for(@user),
                         balances: AccountBalance.for_user(@user).nonzero.includes(:asset).to_a)
  end

  # ── the whole account agrees ─────────────────────────────────────────────────────────────────
  test 'when everything reconciles, every figure is stated and the identity holds' do
    tx(:deposit, day: 1, base_currency: 'USDC', base_amount: 1_000)
    tx(:buy, day: 2, base_currency: 'BTC', base_amount: 1, quote_currency: 'USDC', quote_amount: 1_000)
    balance(@btc, 1, 1_500)

    result = figures

    assert_empty result.findings
    assert_equal 1_000.to_d, result.invested
    assert_equal 1_500.to_d, result.value
    assert_equal 500.to_d, result.unrealised
    assert_equal 500.to_d, result.total
    # The identity the page rests on, asserted rather than hoped for.
    assert_equal result.value - result.invested, result.realised + result.unrealised
  end

  # ── the impossible ───────────────────────────────────────────────────────────────────────────
  test 'a history that goes negative is reported, not floored into a plausible number' do
    # Sold coins that never arrived: the opening balance predates whatever history we hold.
    tx(:sell, day: 1, base_currency: 'BTC', base_amount: 5, quote_currency: 'USDC', quote_amount: 50_000)
    tx(:buy, day: 2, base_currency: 'BTC', base_amount: 1, quote_currency: 'USDC', quote_amount: 20_000)
    balance(@btc, 1, 20_000)

    finding = figures.findings.find { |f| f.kind == :history_incomplete }

    assert finding, "expected a history finding, got #{figures.findings.map(&:kind).inspect}"
    assert_equal 'BTC', finding.symbol
  end

  test 'a withdrawal of coins that never arrived is reported too' do
    tx(:withdrawal, day: 1, base_currency: 'BTC', base_amount: 3)
    tx(:buy, day: 2, base_currency: 'BTC', base_amount: 1, quote_currency: 'USDC', quote_amount: 20_000)
    balance(@btc, 1, 20_000)

    assert_includes figures.findings.map(&:kind), :history_incomplete
  end

  # ── the comparison nobody was making ─────────────────────────────────────────────────────────
  test 'a quantity the venue does not confirm is reported' do
    tx(:deposit, day: 1, base_currency: 'USDC', base_amount: 1_000)
    tx(:buy, day: 2, base_currency: 'BTC', base_amount: 1, quote_currency: 'USDC', quote_amount: 1_000)
    balance(@btc, 0.4, 600) # the venue says we hold far less

    finding = figures.findings.find { |f| f.kind == :quantity_disagrees }

    assert finding
    assert_equal 'BTC', finding.symbol
  end

  test 'a coin the ledger holds that no balance reports at all is reported' do
    tx(:deposit, day: 1, base_currency: 'USDC', base_amount: 1_000)
    tx(:buy, day: 2, base_currency: 'BTC', base_amount: 1, quote_currency: 'USDC', quote_amount: 1_000)
    # no balance row for BTC

    assert_includes figures.findings.map { |f| [f.kind, f.symbol] }, [:quantity_disagrees, 'BTC']
  end

  # Dust rounding is not a disagreement — exchanges and ledgers never match to the last digit.
  test 'a rounding difference is not a finding' do
    tx(:deposit, day: 1, base_currency: 'USDC', base_amount: 1_000)
    tx(:buy, day: 2, base_currency: 'BTC', base_amount: 1, quote_currency: 'USDC', quote_amount: 1_000)
    balance(@btc, 0.999, 1_500)

    assert_empty figures.findings
  end

  # ── what a finding costs the figures ─────────────────────────────────────────────────────────
  test 'a holding we cannot vouch for states no unrealised figure of its own' do
    tx(:deposit, day: 1, base_currency: 'USDC', base_amount: 1_000)
    tx(:buy, day: 2, base_currency: 'BTC', base_amount: 1, quote_currency: 'USDC', quote_amount: 1_000)
    balance(@btc, 0.4, 600)

    holding = figures.holdings.find { |h| h.asset.symbol == 'BTC' }

    assert_nil holding.unrealised, 'a basis nobody can stand behind states no gain'
    assert holding.finding, 'and it says why'
  end

  # ── cash ─────────────────────────────────────────────────────────────────────────────────
  #
  # Cash is a balance, not a position — and it was never compared. The ledger knows what it holds
  # after every deposit, trade and fee; the venue reports what it holds; a dollar the two disagree
  # about is exactly as much a finding as a coin they disagree about.
  test 'cash the sale returned that the venue does not show is a finding, not a mystery' do
    tx(:deposit, day: 1, base_currency: 'USDC', base_amount: 1_000)
    tx(:buy, day: 2, base_currency: 'BTC', base_amount: 1, quote_currency: 'USDC', quote_amount: 1_000)
    tx(:sell, day: 3, base_currency: 'BTC', base_amount: 0.5, quote_currency: 'USDC', quote_amount: 900)
    balance(@btc, 0.5, 800) # ...and the 900 of cash the sale returned is nowhere in the balances

    result = figures

    assert_equal [:cash_disagrees], result.findings.map(&:kind)
    assert_equal 900.to_d, result.findings.sole.detail
    assert_not result.vouched?
  end

  test 'cash the venue reports as the ledger has it is no finding' do
    usdc = create(:asset, symbol: 'USDC', name: 'USD Coin')
    tx(:deposit, day: 1, base_currency: 'USDC', base_amount: 1_000)
    tx(:buy, day: 2, base_currency: 'BTC', base_amount: 1, quote_currency: 'USDC', quote_amount: 600)
    balance(@btc, 1, 700)
    balance(usdc, 400, 400)

    assert_empty figures.findings
  end

  # A balance is a snapshot and the bots go on trading: a fill since the sync moved cash as well as
  # coins, and the venue's figure has to be brought forward on both before the two are compared.
  test 'a fill since the balances were taken moves cash too' do
    usdc = create(:asset, symbol: 'USDC', name: 'USD Coin')
    tx(:deposit, day: 1, base_currency: 'USDC', base_amount: 1_000)
    tx(:buy, day: 2, base_currency: 'BTC', base_amount: 1, quote_currency: 'USDC', quote_amount: 600)
    synced_at = @day.call(3)
    tx(:sell, day: 4, base_currency: 'BTC', base_amount: 0.5, quote_currency: 'USDC', quote_amount: 350)
    balance(@btc, 1, 700)
    balance(usdc, 400, 400)
    pending = Tracker::Figures.moved_since(AccountTransaction.for_user(@user), synced_at)

    result = Tracker::Figures.for(@user, ledger: Tracker::Ledger.for(@user),
                                         balances: AccountBalance.for_user(@user).nonzero.includes(:asset).to_a,
                                         pending: pending)

    assert_equal({ 'BTC' => -0.5.to_d, 'USDC' => 350.to_d }, pending)
    assert_empty result.findings
  end

  # ── the backstop ─────────────────────────────────────────────────────────────────────────
  #
  # Every check above names something the VENUE disagrees with. This one checks the ledger's own
  # arithmetic: what its positions cost plus the cash it holds must be what went in plus what was
  # realised, with no venue in the equation. It runs whether or not anything above fired — a named
  # finding explains a disagreement with the venue, and must never hide a contradiction in the
  # figures themselves.
  test 'figures that do not add up say so, even beside a named finding' do
    broken = Tracker::Ledger::Summary.new(
      positions: [Tracker::Ledger::Position.new(symbol: 'BTC', asset: @btc, quantity: 1.to_d, cost_usd: 1_000.to_d,
                                                avg_cost_usd: 1_000.to_d, opened_at: @day.call(1), incomplete: false)],
      round_trips: [], total_invested_usd: 500.to_d, received_usd: 0.to_d, realised_pnl_usd: 0.to_d,
      fees_usd: 0.to_d, cash_usd: 0.to_d, incomplete: false, overdrawn: {}, computed_at: Time.current
    )
    balance(@btc, 0.4, 600) # the venue disagrees about the quantity...

    result = Tracker::Figures.for(@user, ledger: broken, balances: AccountBalance.for_user(@user).includes(:asset).to_a)

    # ...and the ledger disagrees with itself: 1,000 of cost against 500 of money in and nothing realised.
    assert_equal %i[figures_disagree quantity_disagrees], result.findings.map(&:kind).sort
    assert_equal 500.to_d, result.findings.find { |f| f.kind == :figures_disagree }.detail
  end

  test 'cash is not a position and never a finding' do
    usdc = create(:asset, symbol: 'USDC', name: 'USD Coin')
    tx(:deposit, day: 1, base_currency: 'USDC', base_amount: 1_000)
    balance(usdc, 1_000, 1_000)

    result = figures

    assert_empty result.findings
    assert_equal 1_000.to_d, result.value
  end

  # ── one definition ───────────────────────────────────────────────────────────────────────
  #
  # Invested is denominated in basis: what entered at its value on entry, less what left at the
  # basis it carried. Then, when the venue reports exactly what the ledger holds, the two halves
  # agree to the cent over every kind of row — a reward, a swap, a sale, a withdrawal, a loss.
  test 'when the venue agrees with the ledger, the identity holds over every kind of row' do
    eth = create(:asset, :ethereum)
    bnb = create(:asset, symbol: 'BNB', name: 'BNB')
    usdc = create(:asset, symbol: 'USDC', name: 'USD Coin')
    price('ETH', 3, 300)
    price('BTC', 4, 700)
    price('BNB', 4, 250)
    tx(:deposit, day: 1, base_currency: 'USDC', base_amount: 1_000)
    tx(:buy, day: 2, base_currency: 'BTC', base_amount: 1, quote_currency: 'USDC', quote_amount: 600)
    tx(:staking_reward, day: 3, base_currency: 'ETH', base_amount: 1, quote_currency: nil, quote_amount: nil)
    tx(:swap_out, day: 4, base_currency: 'BTC', base_amount: 0.5, quote_currency: nil, quote_amount: nil, group_id: 'g')
    tx(:swap_in, day: 4, base_currency: 'BNB', base_amount: 2, quote_currency: nil, quote_amount: nil, group_id: 'g')
    tx(:sell, day: 5, base_currency: 'BNB', base_amount: 1, quote_currency: 'USDC', quote_amount: 250)
    tx(:withdrawal, day: 6, base_currency: 'ETH', base_amount: 0.5)
    tx(:lost, day: 7, base_currency: 'BNB', base_amount: 0.5, quote_currency: nil, quote_amount: nil)
    balance(@btc, 0.5, 400)
    balance(eth, 0.5, 200)
    balance(bnb, 0.5, 60)
    balance(usdc, 650, 650)

    result = figures

    assert_empty result.findings
    assert_equal 1_150.to_d, result.invested, '1,000 of cash, 300 of reward, less the 150 the ETH took out'
    assert_equal 25.to_d, result.realised, '100 on the BNB sold, 75 lost with the BNB the venue took'
    assert_equal 135.to_d, result.unrealised
    assert_equal result.value - result.invested, result.realised + result.unrealised
  end

  def price(symbol, day, usd)
    HistoricalPrice.create!(asset: symbol, currency: 'USD', date: @day.call(day).to_date, price: usd)
  end
end
