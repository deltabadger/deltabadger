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

  # The backstop. Every check above names a holding; this one catches what none of them anticipated,
  # so a set of figures that quietly contradict each other can never reach the page again.
  test 'figures that do not add up say so, even when no holding looks wrong' do
    tx(:deposit, day: 1, base_currency: 'USDC', base_amount: 1_000)
    tx(:buy, day: 2, base_currency: 'BTC', base_amount: 1, quote_currency: 'USDC', quote_amount: 1_000)
    tx(:sell, day: 3, base_currency: 'BTC', base_amount: 0.5, quote_currency: 'USDC', quote_amount: 900)
    balance(@btc, 0.5, 800) # ...and the 900 of cash the sale returned is nowhere in the balances

    result = figures

    assert_equal [:figures_disagree], result.findings.map(&:kind)
    assert_not result.vouched?
  end

  test 'cash is not a position and never a finding' do
    usdc = create(:asset, symbol: 'USDC', name: 'USD Coin')
    tx(:deposit, day: 1, base_currency: 'USDC', base_amount: 1_000)
    balance(usdc, 1_000, 1_000)

    result = figures

    assert_empty result.findings
    assert_equal 1_000.to_d, result.value
  end
end
