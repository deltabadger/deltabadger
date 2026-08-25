require 'test_helper'

# Two figures on one page, measuring different things. The chart's headline is the portfolio
# against every dollar put into it — total return, realised losses on things long since sold
# included. The donut's centre is the holdings ring it sits inside: what the coins held RIGHT NOW
# are worth against what they cost. An account that lost money on a coin it no longer holds and is
# up on the bag it kept will read negative above and positive below, and both are correct.
#
# So they must not both be called P/L. That is the whole of this file: the page may not present two
# different measures under one name.
class TrackerPnlLabelsTest < ActionDispatch::IntegrationTest
  setup do
    Tax::EcbFxRates.stubs(:ensure_loaded!)
    Rails.stubs(:cache).returns(ActiveSupport::Cache::MemoryStore.new)
    @user = create(:user, admin: true, setup_completed: true)
    @binance = create(:binance_exchange)
    @key = create(:api_key, user: @user, exchange: @binance)
    @btc = create(:asset, :bitcoin, color: '#F7931A')
    AccountBalance.create!(user: @user, exchange: @binance, asset: @btc, free: 1, locked: 0,
                           usd_price: 50_000, usd_value: 50_000,
                           synced_at: Time.current, priced_at: Time.current)
    create(:account_transaction, api_key: @key, entry_type: :buy, base_currency: 'BTC', base_amount: 1,
                                 quote_currency: 'USD', quote_amount: 40_000, transacted_at: 10.days.ago)
    Tracker::Ledger.compute!(@user)
    sign_in @user
  end

  test 'the donut states the measure it shows, and it is not the chart\'s' do
    get tracker_path

    assert_response :success
    assert_select '.tracker-holdings__centre .label', text: I18n.t('tracker.unrealised')
    assert_select '.tracker-holdings__centre .label', text: I18n.t('data_labels.pnl'), count: 0
  end

  test 'a column of per-row outcomes is still P/L' do
    get tracker_path

    assert_select '.tracker-positions th', text: I18n.t('data_labels.pnl')
  end

  test 'every locale names it' do
    I18n.available_locales.each do |locale|
      assert I18n.exists?('tracker.unrealised', locale),
             "#{locale} has no tracker.unrealised"
    end
  end
end
