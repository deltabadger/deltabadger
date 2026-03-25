require 'test_helper'

class TrackerTest < ActionDispatch::IntegrationTest
  setup do
    @user = create(:user, admin: true, setup_completed: true)
    @exchange = create(:binance_exchange)
    @api_key = create(:api_key, user: @user, exchange: @exchange)
    sign_in @user
  end

  test 'index page loads with connect button when no exchanges connected' do
    user_no_keys = create(:user, setup_completed: true)
    sign_in user_no_keys

    get tracker_index_path
    assert_response :success
    assert_match 'pick_exchange', response.body
  end

  test 'index page loads with table when transactions exist' do
    create(:account_transaction,
           api_key: @api_key,
           exchange: @exchange,
           entry_type: :buy,
           base_currency: 'BTC',
           base_amount: 0.5,
           quote_currency: 'USD',
           quote_amount: 25_000.0,
           transacted_at: 1.day.ago)

    get tracker_index_path
    assert_response :success
    assert_select '.widget--table--tracker'
    assert_select 'td', text: 'BTC'
  end

  test 'filters by exchange' do
    other_exchange = create(:kraken_exchange)
    other_api_key = create(:api_key, user: @user, exchange: other_exchange)

    create(:account_transaction, api_key: @api_key, exchange: @exchange, base_currency: 'BTC', transacted_at: 1.day.ago)
    create(:account_transaction, api_key: other_api_key, exchange: other_exchange, base_currency: 'ETH', transacted_at: 1.day.ago)

    get tracker_index_path(exchange_id: @exchange.id)
    assert_response :success
    assert_select 'td', text: 'BTC'
    assert_select 'td', text: 'ETH', count: 0
  end

  test 'filters by date range' do
    create(:account_transaction, api_key: @api_key, exchange: @exchange, base_currency: 'BTC', transacted_at: 10.days.ago)
    create(:account_transaction, api_key: @api_key, exchange: @exchange, base_currency: 'ETH', transacted_at: 1.day.ago)

    get tracker_index_path(from: 5.days.ago.to_date.iso8601, to: Date.current.iso8601)
    assert_response :success
    assert_select 'td', text: 'ETH'
    assert_select 'td', text: 'BTC', count: 0
  end

  test 'sync enqueues sync jobs' do
    post sync_tracker_index_path
    assert_redirected_to tracker_index_path
    follow_redirect!
    assert_response :success
  end

  test 'export returns CSV file' do
    create(:account_transaction,
           api_key: @api_key,
           exchange: @exchange,
           entry_type: :buy,
           base_currency: 'BTC',
           base_amount: 0.5,
           quote_currency: 'USD',
           quote_amount: 25_000.0,
           transacted_at: Time.utc(2026, 3, 20, 10, 0, 0))

    get export_tracker_index_path
    assert_response :success
    assert_equal 'text/csv; charset=utf-8', response.content_type
    assert_match 'deltabadger-transactions-', response.headers['Content-Disposition']

    lines = response.body.split("\n")
    assert_equal 'date,type,base_currency,base_amount,quote_currency,quote_amount,fee_currency,fee_amount,exchange,tx_id,group_id,description',
                 lines[0]
    assert_includes lines[1], '2026-03-20T10:00:00Z'
    assert_includes lines[1], 'buy'
    assert_includes lines[1], 'BTC'
    assert_includes lines[1], '0.5'
  end

  test 'export respects date filter' do
    create(:account_transaction, api_key: @api_key, exchange: @exchange, base_currency: 'OLD', transacted_at: 30.days.ago)
    create(:account_transaction, api_key: @api_key, exchange: @exchange, base_currency: 'NEW', transacted_at: 1.day.ago)

    get export_tracker_index_path(from: 5.days.ago.to_date.iso8601)
    lines = response.body.split("\n")
    assert_equal 2, lines.length
    assert_includes lines[1], 'NEW'
  end

  test 'export respects exchange filter' do
    other_exchange = create(:kraken_exchange)
    other_api_key = create(:api_key, user: @user, exchange: other_exchange)

    create(:account_transaction, api_key: @api_key, exchange: @exchange, base_currency: 'BTC', transacted_at: 1.day.ago)
    create(:account_transaction, api_key: other_api_key, exchange: other_exchange, base_currency: 'ETH', transacted_at: 1.day.ago)

    get export_tracker_index_path(exchange_id: other_exchange.id)
    lines = response.body.split("\n")
    assert_equal 2, lines.length
    assert_includes lines[1], 'ETH'
  end

  test 'requires authentication' do
    sign_out @user
    get tracker_index_path
    assert_response :redirect
  end
end
