# frozen_string_literal: true

require 'test_helper'

# The tracker with balances hidden. The portfolio is still the whole point of the page, so it stays
# — as allocation. Each holding keeps its share, its colour and its arc of the ring, and states no
# value; the transactions table follows the same rule the bot log does.
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

  # The figure tiles are money and nothing else, so the whole grid goes — the bot's own rule.
  test 'the portfolio figures are not stated' do
    stub_portfolio

    get tracker_path

    assert_select '.data-grid', false
  end

  test 'the portfolio figures are stated when balances are shown' do
    @user.update!(hide_balances: false)
    stub_portfolio

    get tracker_path

    assert_select '.data-grid__item__value', text: /\$40,000\.00/
  end

  test 'each holding keeps its share and states no value' do
    stub_portfolio

    get tracker_path

    assert_select '.tracker-holdings__pct', text: '75.0%'
    assert_select '.tracker-holdings__pct', text: '25.0%'
    assert_select '.tracker-holdings__value', false
  end

  # The ring only ever needed proportions, and it is drawn server-side — so there is no payload to
  # keep the portfolio out of, only geometry.
  test 'the ring is drawn either way and carries no figures' do
    stub_portfolio

    get tracker_path

    assert_select '.tracker-holdings__ring svg circle', 2
    assert_no_match(/30000|30,000|10,000/, response.body)
  end

  test 'a transaction row keeps its date, type and currencies' do
    create(:account_transaction, api_key: @api_key, exchange: @exchange, entry_type: :buy,
                                 base_currency: 'BTC', base_amount: 0.5,
                                 quote_currency: 'USD', quote_amount: 25_000.0,
                                 fee_amount: 12.5, fee_currency: 'USD', transacted_at: 1.day.ago)

    get tracker_path

    assert_select 'td', text: 'BTC'
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

    assert_not column?('amount')
    assert_not column?('quote_amount')
    assert_not column?('fee')
    assert column?('date')
    assert column?('base')
    # A price is a market level, not a balance — the bot log keeps it too.
    assert column?('price')
    assert column?('bot')
  end

  test 'the table keeps every column when balances are shown' do
    @user.update!(hide_balances: false)
    create(:account_transaction, api_key: @api_key, exchange: @exchange, base_currency: 'BTC',
                                 transacted_at: 1.day.ago)

    get tracker_path

    assert column?('amount')
    assert column?('quote_amount')
    assert column?('fee')
  end
  # The money columns name their unit in the header — "Value $" — so a column is matched by the
  # label it starts with rather than by the whole cell.
  def column?(key)
    css_select('.tracker-record__table thead th')
      .any? { |th| th.text.strip.start_with?(I18n.t("tracker.columns.#{key}")) }
  end
end
