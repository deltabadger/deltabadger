# frozen_string_literal: true

require 'test_helper'

# The tracker with balances hidden. The portfolio is still the whole point of the page, so it stays
# — as allocation. Each holding keeps its share, its colour and its slice of the donut, and states
# no value; the transactions table follows the same rule the bot log does.
class TrackerHideBalancesTest < ActionDispatch::IntegrationTest
  setup do
    @user = create(:user, admin: true, setup_completed: true, hide_balances: true)
    @exchange = create(:binance_exchange)
    @api_key = create(:api_key, user: @user, exchange: @exchange)
    sign_in @user
  end

  # 30,000 of BTC beside 10,000 of ETH: a 75/25 split, so the shares are unmistakable in the markup
  # whichever way they are expressed.
  def stub_portfolio
    AccountBalance.create!(user: @user, exchange: @exchange, asset: create(:asset, :bitcoin),
                           free: 1, usd_price: 30_000, usd_value: 30_000, synced_at: Time.current)
    AccountBalance.create!(user: @user, exchange: @exchange,
                           asset: create(:asset, symbol: 'ETH', name: 'Ethereum', external_id: 'ethereum'),
                           free: 5, usd_price: 2_000, usd_value: 10_000, synced_at: Time.current)
  end

  test 'the portfolio total is not stated' do
    stub_portfolio

    get tracker_path

    assert_select '.dash-intro .header--1', false
  end

  test 'the portfolio total is stated when balances are shown' do
    @user.update!(hide_balances: false)
    stub_portfolio

    get tracker_path

    assert_select '.dash-intro .header--1'
  end

  test 'each holding keeps its share and states no value' do
    stub_portfolio

    get tracker_path

    assert_select '.tracker-portfolio__asset .allocation', text: '75.0%'
    assert_select '.tracker-portfolio__asset .allocation', text: '25.0%'
    assert_select '.tracker-portfolio__asset-value', false
  end

  # The donut only ever needed proportions. Handing it shares rather than dollars keeps the
  # portfolio out of the data attribute as well as off the page.
  test 'the donut is handed shares rather than values' do
    stub_portfolio

    get tracker_path

    data = JSON.parse(css_select('[data-donut-chart-data-value]').first['data-donut-chart-data-value'])
    assert_equal([75.0, 25.0], data.map { |slice| slice['value'] })
  end

  test 'the donut is handed values when balances are shown' do
    @user.update!(hide_balances: false)
    stub_portfolio

    get tracker_path

    data = JSON.parse(css_select('[data-donut-chart-data-value]').first['data-donut-chart-data-value'])
    assert_equal([30_000.0, 10_000.0], data.map { |slice| slice['value'] })
  end

  test 'a transaction row keeps its date, type and currencies' do
    create(:account_transaction, api_key: @api_key, exchange: @exchange, entry_type: :buy,
                                 base_currency: 'BTC', base_amount: 0.5,
                                 quote_currency: 'USD', quote_amount: 25_000.0,
                                 fee_amount: 12.5, fee_currency: 'USD', transacted_at: 1.day.ago)

    get tracker_path

    assert_select 'td', text: 'BTC'
    assert_select 'td', text: 'USD'
    assert_select 'td', text: I18n.t('tracker.types.buy')
  end

  # Same rule as the bot log: the amount held, what it cost, and the fee paid are all money.
  test 'a transaction row states no amount, no value and no fee' do
    at = create(:account_transaction, api_key: @api_key, exchange: @exchange, entry_type: :buy,
                                      base_currency: 'BTC', base_amount: 0.5,
                                      quote_currency: 'USD', quote_amount: 25_000.0,
                                      fee_amount: 12.5, fee_currency: 'USD', transacted_at: 1.day.ago)

    get tracker_path

    cells = css_select("##{ActionView::RecordIdentifier.dom_id(at)} td").map { |td| td.text.strip }
    assert_not_includes cells.join(' '), '0.5'
    assert_not_includes cells.join(' '), '25000'
    assert_not_includes cells.join(' '), '12.5'
  end

  test 'the transactions table drops those three columns and keeps the rest' do
    create(:account_transaction, api_key: @api_key, exchange: @exchange, base_currency: 'BTC',
                                 transacted_at: 1.day.ago)

    get tracker_path

    headers = css_select('.widget--table--tracker thead th').map { |th| th.text.strip }
    assert_not_includes headers, I18n.t('tracker.columns.amount')
    assert_not_includes headers, I18n.t('tracker.columns.quote_amount')
    assert_not_includes headers, I18n.t('tracker.columns.fee')
    assert_includes headers, I18n.t('tracker.columns.date')
    assert_includes headers, I18n.t('tracker.columns.base')
    assert_includes headers, I18n.t('tracker.columns.quote')
    assert_includes headers, I18n.t('tracker.columns.bot')
  end

  test 'the table keeps every column when balances are shown' do
    @user.update!(hide_balances: false)
    create(:account_transaction, api_key: @api_key, exchange: @exchange, base_currency: 'BTC',
                                 transacted_at: 1.day.ago)

    get tracker_path

    headers = css_select('.widget--table--tracker thead th').map { |th| th.text.strip }
    assert_includes headers, I18n.t('tracker.columns.amount')
    assert_includes headers, I18n.t('tracker.columns.quote_amount')
    assert_includes headers, I18n.t('tracker.columns.fee')
  end
end
