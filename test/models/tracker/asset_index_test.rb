require 'test_helper'

# What the tracker draws a SYMBOL with — the logo, the colour, the full name — for every row in
# the transactions table and every position in the ledger.
#
# The account's own balance row wins, because that is the instrument this account actually holds.
# Everything else falls back to the catalog, and the fallback used to ask for crypto ONLY: a
# EUR row on the page resolved to nothing at all, so the transactions table drew a currency with
# no logo, no colour and no name while the catalog held all three.
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
end
