require 'test_helper'

# A currency draws its flag on the tracker, from this repo's own assets.
#
# Two ways it used to come out blank, both fixed here. USD resolved to the local `usd` row the
# Alpaca sync keeps — an asset no catalog has a picture for — and EUR resolved to nothing at all,
# because the fallback asked for crypto only. Either way the row rendered logo-less.
class TrackerFiatLogoTest < ActionDispatch::IntegrationTest
  setup do
    MarketData.stubs(:configured?).returns(true)
    Tax::EcbFxRates.stubs(:ensure_loaded!)
    Rails.stubs(:cache).returns(ActiveSupport::Cache::MemoryStore.new)
    @user = create(:user, admin: true, setup_completed: true)
    @binance = create(:binance_exchange)
    @key = create(:api_key, user: @user, exchange: @binance)
    @btc = create(:asset, :bitcoin, color: '#F7931A')
    # The dollar the account actually holds: the local row, with no image of its own.
    @usd = create(:asset, :usd)
    # A currency in the catalog that the account holds no balance in.
    create(:asset, external_id: 'EUR.FOREX', symbol: 'EUR', name: 'Euro', category: 'Currency', color: '#003087')
    @t = Time.utc(2026, 8, 1, 12)
    sign_in @user
  end

  def tx(type, currency, amount)
    create(:account_transaction, api_key: @key, entry_type: type, base_currency: currency, base_amount: amount,
                                 quote_currency: nil, quote_amount: nil, transacted_at: @t)
  end

  test 'a dollar row draws the US flag' do
    AccountBalance.create!(user: @user, exchange: @binance, asset: @usd, free: 500, locked: 0, usd_price: 1,
                           usd_value: 500, synced_at: Time.current, priced_at: Time.current)
    tx(:deposit, 'USD', 500)
    Tracker::Ledger.compute!(@user)

    get tracker_path
    assert_response :success
    # Matched on the file, not the full path: sprockets digests it.
    assert_select 'img.asset-logo[src*=?]', 'flags/us'
  end

  test 'a euro row draws the EU flag, with no balance in it and nothing fetched' do
    tx(:deposit, 'EUR', 250)
    Tracker::Ledger.compute!(@user)

    get tracker_path
    assert_response :success
    assert_select 'img.asset-logo[src*=?]', 'flags/eu'
  end

  test 'a coin still draws its own image, not a flag' do
    @btc.update!(image_url: 'https://coin-images.coingecko.com/coins/images/1/large/bitcoin.png')
    tx(:deposit, 'BTC', 1)
    Tracker::Ledger.compute!(@user)

    get tracker_path
    assert_response :success
    assert_select 'img.asset-logo[src=?]', @btc.image_url
    assert_select 'img.asset-logo[src*=?]', 'flags/', count: 0
  end
end
