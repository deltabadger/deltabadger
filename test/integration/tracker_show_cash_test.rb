require 'test_helper'

# "Show cash" — the tracker's second scope control, beside the venue switch.
#
# Cash and stablecoins are where money WAITS; they are not a position anybody picked. A portfolio
# that is a fifth USDT gives its second-biggest arc to a coin nobody chose, so by default the
# allocation is drawn from what was actually invested and the rest is normalized to 100%.
#
# It scopes the page to that: the list, and the figures that describe it. With cash hidden, money
# in is what the holdings left over COST, the value is what that portion is worth now, and the P/L
# between them is the one still riding. Fees paid and gains banked are the account's record of what
# it has already done and stay whole either way — as does the curve above them, which is stored
# history and knows nothing of the switch.
class TrackerShowCashTest < ActionDispatch::IntegrationTest
  setup do
    Tax::EcbFxRates.stubs(:ensure_loaded!)
    Rails.stubs(:cache).returns(ActiveSupport::Cache::MemoryStore.new)
    @user = create(:user, admin: true, setup_completed: true)
    @binance = create(:binance_exchange)
    @key = create(:api_key, user: @user, exchange: @binance)
    @btc = create(:asset, :bitcoin, color: '#F7931A')
    @usdt = create(:asset, :usdt)
    # 40,000 in, 30,000 of it spent on 1 BTC now worth 36,000. 10,000 USDT still waiting.
    AccountBalance.create!(user: @user, exchange: @binance, asset: @btc, free: 1, locked: 0,
                           usd_price: 36_000, usd_value: 36_000, synced_at: Time.current, priced_at: Time.current)
    AccountBalance.create!(user: @user, exchange: @binance, asset: @usdt, free: 10_000, locked: 0,
                           usd_price: 1, usd_value: 10_000, synced_at: Time.current, priced_at: Time.current)
    create(:account_transaction, api_key: @key, exchange: @binance, entry_type: :deposit,
                                 base_currency: 'USDT', base_amount: 40_000, quote_currency: nil,
                                 quote_amount: nil, transacted_at: 10.days.ago)
    create(:account_transaction, api_key: @key, exchange: @binance, entry_type: :buy,
                                 base_currency: 'BTC', base_amount: 1, quote_currency: 'USDT',
                                 quote_amount: 30_000, transacted_at: 9.days.ago)
    Tracker::Ledger.compute!(@user)
    sign_in @user
  end

  def show_cash!
    @user.update!(tracker_settings: (@user.tracker_settings || {}).merge('show_cash' => true))
  end

  def tiles
    css_select('.data-grid .data-grid__item').to_h do |item|
      # The label's own text: a tile with a hint carries the whole hint sentence inside `.label`
      # too, and the figure beside the name is what is being compared here.
      [item.at_css('.label').xpath('./text()').text.strip,
       item.css('.data-grid__item__value').text.strip]
    end
  end

  # ---- the control itself -------------------------------------------------

  # The same switch the account menu uses, in the row that already says what the page is scoped to.
  # Packed left in the scope row, directly after the venue switch, and the switch itself reads
  # first — the control, then what it is for.
  test 'the toggle rides in the scope row after the venue switch, small, and off by default' do
    get tracker_path

    assert_select '.tracker-exchanges > .filters > form.tracker-exchanges__switch' do
      assert_select 'label > *:first-child.toggle.toggle--small'
      assert_select 'label > span:last-child', text: I18n.t('tracker.show_cash')
      assert_select 'input[type=hidden][name=show_cash][value="0"]'
      assert_select '.toggle.toggle--small input[type=checkbox][name=show_cash]:not([checked])'
    end
  end

  test 'the toggle reads back on once the preference is set' do
    show_cash!

    get tracker_path

    assert_select '.toggle.toggle--small input[type=checkbox][name=show_cash][checked]'
  end

  # Posted state, not a flip — the account menu's rule: two quick clicks must not land out of order.
  test 'the switch stores what was submitted and comes back to the page it was thrown on' do
    patch settings_update_show_cash_path, params: { show_cash: '1' }, headers: { 'HTTP_REFERER' => tracker_path }

    assert_redirected_to tracker_path
    assert @user.reload.tracker_settings['show_cash']

    patch settings_update_show_cash_path, params: { show_cash: '0' }, headers: { 'HTTP_REFERER' => tracker_path }

    assert_not @user.reload.tracker_settings['show_cash']
  end

  # ---- what it hides ------------------------------------------------------

  test 'cash is off the holdings card by default and back on it when asked' do
    get tracker_path

    assert_select '.tracker-holdings__head .label', text: /#{I18n.t('tracker.holdings')} · 1/
    assert_select '.tracker-holdings__rows .tracker-holdings__row', 1
    assert_select '.tracker-holdings__name b', text: 'USDT', count: 0

    show_cash!
    get tracker_path

    assert_select '.tracker-holdings__head .label', text: /#{I18n.t('tracker.holdings')} · 2/
    assert_select '.tracker-holdings__name b', text: 'USDT', count: 1
  end

  # The ring is drawn from the same list, so what the card leaves out the ring cannot draw: with
  # cash hidden BTC is the whole allocation, not 78% of it.
  test 'the allocation is normalized to what is left' do
    get tracker_path

    assert_select '.tracker-holdings__pct', text: '100.0%'

    show_cash!
    get tracker_path

    assert_select '.tracker-holdings__pct', text: '78.3%'
    assert_select '.tracker-holdings__pct', text: '21.7%'
  end

  # The positions table lists what is HELD, so it lists cash too — and hiding a balance hides it
  # wherever the page states it, not only on the card.
  test 'the positions table drops the cash row with it' do
    get tracker_path

    assert_select '.tracker-positions .tracker-row__asset', text: /USDT/, count: 0

    show_cash!
    get tracker_path

    assert_select '.tracker-positions .tracker-row__asset', text: /USDT/, count: 1
  end

  # ---- what it restates ---------------------------------------------------

  # 40,000 came in, 30,000 of it bought the BTC now worth 36,000, and 10,000 is still waiting. With
  # the cash hidden the page is about the 30,000 that was put to work and what it has become.
  test 'money in and value follow the switch' do
    get tracker_path
    hidden = tiles

    assert_equal '$30,000.00', hidden[I18n.t('bot.details.stats.total_invested')]
    assert_equal '$36,000.00', hidden[I18n.t('bot.details.stats.portfolio_value')]
    assert_equal '+$6,000.00', hidden[I18n.t('tracker.tiles.total_pnl')]

    show_cash!
    get tracker_path
    shown = tiles

    assert_equal '$40,000.00', shown[I18n.t('bot.details.stats.total_invested')]
    assert_equal '$46,000.00', shown[I18n.t('bot.details.stats.portfolio_value')]
    assert_equal '+$6,000.00', shown[I18n.t('tracker.tiles.total_pnl')]
  end

  # ---- what it must not touch ---------------------------------------------

  # Hiding a balance does not unspend a fee or unbank a gain.
  test 'fees and what was realised are the whole account either way' do
    get tracker_path
    hidden = tiles

    show_cash!
    get tracker_path

    labels = [I18n.t('tracker.fees_paid'), I18n.t('bot.dca_index.realised_pnl')]

    assert_equal tiles.values_at(*labels), hidden.values_at(*labels)
  end

  # The curve is the same reading as the tiles under it: the whole portfolio with cash shown, the
  # positions inside it with cash hidden. Both pairs are on every day, so the switch picks a column.
  test 'the curve follows the switch' do
    (1..3).each do |n|
      PortfolioSnapshot.create!(user: @user, date: Date.current - (400 - (n * 100)),
                                value_usd: 40_000 + (n * 1_000), invested_usd: 40_000,
                                held_value_usd: 30_000 + (n * 1_000), held_cost_usd: 30_000, partial: false)
    end
    series = lambda do
      get tracker_path
      JSON.parse(css_select('[data-controller="bot--chart"]').first['data-bot--chart-series-value'])
    end

    assert_equal [[31_000.0, 32_000.0, 33_000.0], [30_000.0] * 3], series.call

    show_cash!

    assert_equal [[41_000.0, 42_000.0, 43_000.0], [40_000.0] * 3], series.call
  end

  # A history swept before the curve learned about the switch has only the whole-portfolio pair,
  # and no reading of it can recover the cash standing on those days. It is swept again rather than
  # drawn as something it is not — and with cash shown there is nothing missing to wait for.
  test 'a history from before the switch is swept again rather than drawn wrong' do
    # Dated at the first transaction, so the only reason to sweep is the missing pair.
    PortfolioSnapshot.create!(user: @user, date: 10.days.ago.to_date, value_usd: 46_000,
                              invested_usd: 40_000, partial: false)
    PortfolioSnapshot::BackfillJob.expects(:perform_later).with(@user.id, nil).once

    get tracker_path

    assert_select '.widget--chart__plot .loader'
    assert_select '[data-controller="bot--chart"]', false

    show_cash!
    get tracker_path

    assert_select '[data-controller="bot--chart"]'
  end

  # An account holding nothing BUT cash has balances; it just has no invested position to draw.
  # Saying "no balances found" there is a lie the reader cannot act on — the money is right
  # beside it in the tiles — so the empty list has to name the switch that is hiding it.
  test 'an all-cash portfolio says the balances are hidden, not missing' do
    AccountBalance.where(user: @user, asset: @btc).destroy_all
    Tracker::Ledger.compute!(@user)

    get tracker_path

    assert_response :success
    assert_select '.tracker-holdings__empty', text: I18n.t('tracker.portfolio.only_cash')
    assert_select '.tracker-holdings__empty', text: I18n.t('tracker.portfolio.no_balances'), count: 0
  end

  test 'showing cash gives an all-cash portfolio its holdings back' do
    AccountBalance.where(user: @user, asset: @btc).destroy_all
    Tracker::Ledger.compute!(@user)
    show_cash!

    get tracker_path

    assert_response :success
    assert_select '.tracker-holdings__empty', count: 0
    assert_select '.tracker-row__asset', text: /USDT/
  end
end
