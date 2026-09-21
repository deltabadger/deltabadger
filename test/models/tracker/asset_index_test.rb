require 'test_helper'

# What the tracker draws a SYMBOL with — the logo, the colour, the full name — for every position in
# the ledger.
#
# The asset the symbol's rows recorded wins: the venue that booked them said which instrument it was.
# A symbol no row identifies is read by its string as before: the account's own balance row first,
# because that is the instrument this account actually holds, then the catalog — and that fallback
# used to ask for crypto ONLY: a EUR row on the page resolved to nothing at all, so the transactions
# table drew a currency with no logo, no colour and no name while the catalog held all three.
class Tracker::AssetIndexTest < ActiveSupport::TestCase
  setup do
    @user = create(:user)
    @binance = create(:binance_exchange)
    @key = create(:api_key, user: @user, exchange: @binance)
  end

  def balance(asset)
    AccountBalance.create!(user: @user, exchange: @binance, asset: asset, free: 1, locked: 0,
                           usd_price: 1, usd_value: 1, synced_at: Time.current, priced_at: Time.current)
  end

  test 'a currency nobody holds resolves from the catalog' do
    eur = create(:asset, external_id: 'EUR.FOREX', symbol: 'EUR', name: 'Euro', category: 'Currency')

    assert_equal eur, Tracker::Ledger.asset_index(@user, %w[EUR])['EUR']
  end

  test 'the account\'s own balance row still wins over the catalog' do
    create(:asset, external_id: 'USD.FOREX', symbol: 'USD', name: 'US Dollar', category: 'Currency')
    held = create(:asset, :usd)
    balance(held)

    assert_equal held, Tracker::Ledger.asset_index(@user, %w[USD])['USD'],
                 'the row the rest of the page draws this symbol with'
  end

  # The reason the fallback was ever restricted: a stock and a coin can share a ticker, and the
  # crypto one is what a crypto row means. A currency is not a third contender for that ticker.
  test 'a stock sharing a ticker with a coin still loses to the coin' do
    create(:asset, external_id: 'XYZ.US', symbol: 'XYZ', name: 'Block Inc', category: 'Stock')
    coin = create(:asset, external_id: 'xyo-network', symbol: 'XYZ', name: 'XYO', category: 'Cryptocurrency')

    assert_equal coin, Tracker::Ledger.asset_index(@user, %w[XYZ])['XYZ']
  end

  test 'a symbol in neither the balances nor the catalog resolves to nothing' do
    assert_nil Tracker::Ledger.asset_index(@user, %w[NOPE])['NOPE']
  end

  # ── what the rows recorded ─────────────────────────────────────────────────────────────────────

  def alpaca_key = @alpaca_key ||= create(:api_key, user: @user, exchange: create(:alpaca_exchange))

  def recorded(key, symbol, asset, entry_type: :buy)
    create(:account_transaction, api_key: key, entry_type: entry_type, base_currency: symbol, base_asset_id: asset&.id)
  end

  # Bought and sold again: no balance is left to say what it was, and a stock is never in the
  # fallback catalog. Its rows still know.
  test 'a fully sold stock resolves to the asset its rows recorded' do
    create(:asset, external_id: 'SNPS.TO', symbol: 'SNPS', category: 'Stock')
    snps = create(:asset, external_id: 'SNPS.US', symbol: 'SNPS', category: 'Stock')
    recorded(alpaca_key, 'SNPS', snps)
    recorded(alpaca_key, 'SNPS', snps, entry_type: :sell)

    assert_equal snps, Tracker::Ledger.asset_index(@user, %w[SNPS])['SNPS']
  end

  test 'a sold stock whose ticker a coin shares is drawn as the stock' do
    create(:asset, external_id: 'dash', symbol: 'DASH', category: 'Cryptocurrency')
    doordash = create(:asset, external_id: 'DASH.US', symbol: 'DASH', category: 'Stock')
    recorded(alpaca_key, 'DASH', doordash)

    assert_equal doordash, Tracker::Ledger.asset_index(@user, %w[DASH])['DASH']
  end

  # The ledger merges one symbol across venues into one position. When the venues booked two
  # different instruments under it, drawing either would state something about the other.
  test 'a symbol whose rows recorded two assets resolves to none, and in one venue\'s scope to that venue\'s' do
    coin = create(:asset, external_id: 'dash', symbol: 'DASH', category: 'Cryptocurrency')
    doordash = create(:asset, external_id: 'DASH.US', symbol: 'DASH', category: 'Stock')
    balance(coin)
    recorded(@key, 'DASH', coin)
    recorded(alpaca_key, 'DASH', doordash)

    assert_nil Tracker::Ledger.asset_index(@user, %w[DASH])['DASH']
    assert_equal doordash, Tracker::Ledger.asset_index(@user, %w[DASH], exchange: alpaca_key.exchange)['DASH']
    assert_equal coin, Tracker::Ledger.asset_index(@user, %w[DASH], exchange: @binance)['DASH']
  end

  test 'rows without an asset do not outvote rows with one' do
    snps = create(:asset, external_id: 'SNPS.US', symbol: 'SNPS', category: 'Stock')
    recorded(alpaca_key, 'SNPS', nil)
    recorded(alpaca_key, 'SNPS', snps)

    assert_equal snps, Tracker::Ledger.asset_index(@user, %w[SNPS])['SNPS']
  end

  # ── what one round trip is drawn with ─────────────────────────────────────────────────────────

  def trip(symbol, opened_at, closed_at)
    Tracker::Ledger::RoundTrip.new(symbol: symbol, opened_at: opened_at, closed_at: closed_at, quantity: 1.to_d,
                                   invested_usd: 1.to_d, proceeds_usd: 1.to_d, fees_usd: 0.to_d,
                                   realised_pnl_usd: 0.to_d, incomplete: false)
  end

  def recorded_at(key, symbol, asset, at, entry_type: :buy)
    create(:account_transaction, api_key: key, entry_type: entry_type, base_currency: symbol, base_asset_id: asset&.id,
                                 transacted_at: at)
  end

  # A symbol can be two instruments over an account's life: the Dash coin in older rows, DoorDash
  # bought and sold later. The later trip traded one of them, and its own rows say which.
  test 'a round trip is drawn with what its own rows recorded, not what the symbol meant before' do
    coin = create(:asset, external_id: 'dash', symbol: 'DASH', category: 'Cryptocurrency')
    doordash = create(:asset, external_id: 'DASH.US', symbol: 'DASH', category: 'Stock')
    recorded_at(@key, 'DASH', coin, Time.utc(2021, 3, 1), entry_type: :other_income)
    recorded_at(@key, 'DASH', coin, Time.utc(2021, 3, 10), entry_type: :swap_out)
    bought = Time.utc(2025, 6, 2, 14)
    recorded_at(alpaca_key, 'DASH', doordash, bought)
    recorded_at(alpaca_key, 'DASH', doordash, bought + 3.minutes, entry_type: :sell)
    stock_trip = trip('DASH', bought, bought + 3.minutes)
    coin_trip = trip('DASH', Time.utc(2021, 3, 1), Time.utc(2021, 3, 10))

    assets = Tracker::Ledger.trip_assets(@user, [stock_trip, coin_trip])

    assert_equal doordash, assets[stock_trip]
    assert_equal coin, assets[coin_trip]
  end

  test 'a round trip whose own rows recorded two assets is drawn as neither' do
    coin = create(:asset, external_id: 'dash', symbol: 'DASH', category: 'Cryptocurrency')
    doordash = create(:asset, external_id: 'DASH.US', symbol: 'DASH', category: 'Stock')
    at = Time.utc(2025, 6, 2, 14)
    recorded_at(@key, 'DASH', coin, at)
    recorded_at(alpaca_key, 'DASH', doordash, at + 1.minute)
    merged = trip('DASH', at, at + 1.hour)

    assert_nil Tracker::Ledger.trip_assets(@user, [merged])[merged]
  end

  test 'a round trip whose rows recorded nothing is drawn as its symbol is' do
    coin = create(:asset, external_id: 'xyo-network', symbol: 'XYZ', category: 'Cryptocurrency')
    at = Time.utc(2025, 6, 2, 14)
    recorded_at(@key, 'XYZ', nil, at)
    unrecorded = trip('XYZ', at, at + 1.hour)

    assert_equal coin, Tracker::Ledger.trip_assets(@user, [unrecorded])[unrecorded]
  end
end
