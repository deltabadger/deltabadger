require 'test_helper'

# The redesigned /tracker: the bot page's shape (chart head, figure tiles, a filter bar, a card, a
# record bar, a table) built from the bot page's own partials and classes. These tests pin the
# structure and the reuse — not pixel values.
class TrackerPageTest < ActionDispatch::IntegrationTest
  setup do
    Tax::EcbFxRates.stubs(:ensure_loaded!)
    Rails.stubs(:cache).returns(ActiveSupport::Cache::MemoryStore.new)
    @user = create(:user, admin: true, setup_completed: true)
    @binance = create(:binance_exchange)
    @kraken = create(:kraken_exchange)
    @key_binance = create(:api_key, user: @user, exchange: @binance)
    @key_kraken = create(:api_key, user: @user, exchange: @kraken)
    @btc = create(:asset, :bitcoin, color: '#F7931A')
    @eth = create(:asset, :ethereum, color: '#4B507B')
    balance(@binance, @btc, free: 1.5, price: 40_000)
    balance(@kraken, @eth, free: 10, price: 2_000)
    @t = Time.utc(2026, 8, 1, 12)
    create(:account_transaction, api_key: @key_binance, entry_type: :deposit, base_currency: 'USD', base_amount: 30_000,
                                 quote_currency: nil, quote_amount: nil, transacted_at: @t - 10.days)
    # 1.5 BTC for 30,000 plus a 20 USD fee: basis 30,020 → avg 20,013.33, marked at 40k → +99.9%
    create(:account_transaction, api_key: @key_binance, entry_type: :buy, base_currency: 'BTC', base_amount: 1.5,
                                 quote_currency: 'USD', quote_amount: 30_000, transacted_at: @t - 9.days, fee_amount: 20, fee_currency: 'USD')
    create(:account_transaction, api_key: @key_kraken, entry_type: :buy, base_currency: 'ETH', base_amount: 2,
                                 quote_currency: 'USD', quote_amount: 4_000, transacted_at: @t - 8.days)
    create(:account_transaction, api_key: @key_kraken, entry_type: :sell, base_currency: 'ETH', base_amount: 2,
                                 quote_currency: 'USD', quote_amount: 5_000, transacted_at: @t - 7.days)
    @withdrawal = create(:account_transaction, api_key: @key_binance, entry_type: :withdrawal, base_currency: 'USD',
                                               base_amount: 1_000, quote_currency: nil, quote_amount: nil, transacted_at: @t - 6.days)
    sign_in @user
  end

  def balance(exchange, asset, free:, price:)
    AccountBalance.create!(user: @user, exchange: exchange, asset: asset, free: free, locked: 0,
                           usd_price: price, usd_value: free * price, synced_at: Time.current, priced_at: Time.current)
  end

  def warm_ledger = Tracker::Ledger.compute!(@user)

  # ── 1 · chart head: the bot chart's head, verbatim ──────────────────────────────────────────
  test 'a history not built yet spins in the bot\'s empty head; with nothing to build there is no chart' do
    warm_ledger
    get tracker_path
    assert_response :success

    assert_select '.widget--chart .widget--chart__head' do
      assert_select '.widget--chart__summary .widget--chart__date'
    end
    assert_select '.widget--chart .segmented', false, 'no mode switch before there is a series'
    assert_select '.widget--chart__plot .loader'
    assert_select '.widget--chart__plot .widget__placeholder', false, 'a spinner, not an empty landscape'
    assert_select '[data-controller="bot--chart"]', false

    AccountTransaction.for_user(@user).delete_all
    get tracker_path

    assert_select '.widget--chart', false, 'nothing to draw and nothing being built: no chart to promise'
    assert_select '.dash-intro', false, 'the steel headline strip is gone — the chart head is the headline'
  end

  # ── 2 · figure tiles: the bot metrics grid ──────────────────────────────────────────────────
  # Six, not four: the total and the unrealised figure used to live on the chart and in the donut —
  # three scopes in two units in three places, which no reader could add up.
  test 'six figure tiles reuse the bot metrics data-grid with plain-language labels' do
    warm_ledger
    get tracker_path

    assert_select '.widget.data-grid .data-grid__item', 6
    assert_select '.data-grid__item .label', text: I18n.t('bot.details.stats.total_invested')
    assert_select '.data-grid__item .label', text: I18n.t('bot.details.stats.portfolio_value')
    assert_select '.data-grid__item .label', text: I18n.t('bot.dca_index.realised_pnl')
    assert_select '.data-grid__item .label', text: /#{I18n.t('tracker.tiles.unrealised_pnl')}/
    assert_select '.data-grid__item .label', text: I18n.t('tracker.fees_paid')
    assert_select '.data-grid__item .label', text: /#{I18n.t('tracker.tiles.total_pnl')}/
    # 30,000 in − 1,000 out + the 5,020 each venue spent without reporting its arrival = 34,020; then
    # the resolution against the balances: 10 ETH the exchange holds with no history behind them,
    # taken as arrived at its price (+20,000), and the sale's 5,000 the exchange no longer shows,
    # taken as moved out (−5,000).
    assert_select '.data-grid__item__value', text: /\$49,020\.00/
    assert_select '.data-grid__item__value', text: /\$80,000\.00/
    assert_select '.data-grid__item__value.text-success', text: /1,000\.00/ # the ETH round-trip
    assert_select '.data-grid__item__value', text: /\$20\.00/
    assert_no_match(/translation_missing/, response.body)
  end

  test 'a cold ledger shows the bot\'s loading placeholder in the ledger tiles and warms itself once' do
    Tracker::LedgerJob.expects(:perform_later).with(@user.id, nil).once
    get tracker_path

    assert_select '.data-grid__item .loader--small', 5,
                  'portfolio value is known from balances; the other five wait for the ledger ' \
                  '(bots/_metrics_item loading state)'
  end

  # ── 3 · exchange bar ─────────────────────────────────────────────────────────────────────────
  test 'the exchange switch is a link segmented with the scope current, a + to connect, and the sync state' do
    warm_ledger
    create(:api_key, :incorrect, user: @user, exchange: create(:bitget_exchange))
    get tracker_path

    assert_select '.tracker-exchanges .filters > .segmented' do
      assert_select 'a.segmented__option[aria-current="page"]', text: I18n.t('tracker.filters.all')
      assert_select 'a.segmented__option', text: 'Binance'
      assert_select 'a.segmented__option', text: 'Kraken'
      assert_select 'a.segmented__option[data-broken][data-turbo-frame="modal"]', text: /Bitget/
    end
    # The + belongs to the venues it adds to, so it rides in the switch as the last thing in the
    # menu — at the end of the track, at the foot of the list once the control has folded.
    assert_select ".segmented__menu > *:last-child.segmented__option--action[href='#{new_tracker_pick_exchange_path}'][data-turbo-frame='modal']", 1
    assert_select '.tracker-exchanges .dropdown--exchanges', 0, 'one destination now: the picker modal'
    # And with it inside, the switch is its parent's ONLY child again — which is what lets the
    # control fold honestly. A sibling in that box is room the switch is told it can grow into
    # while something already stands there, and the row runs over the sync state instead.
    assert_select '.tracker-exchanges > .filters' do
      assert_select '> *', 1, 'the box the switch measures holds the switch and nothing else'
      assert_select '> .segmented', 1
    end
    assert_select '.tracker-exchanges .tracker-sync', text: /ago/i
    assert_select ".tracker-exchanges form[action='#{sync_tracker_path}'] .rbutton"
  end

  # ── 4 · holdings card ────────────────────────────────────────────────────────────────────────
  test 'holdings: a flat ring and rows with logo, symbol, type tag, allocation, value and P/L' do
    warm_ledger
    get tracker_path

    assert_select '.widget.tracker-holdings' do
      assert_select '.tracker-holdings__ring svg path', 2
      assert_select '.tracker-holdings__row', 2
      assert_select '.tracker-holdings__row:first-child' do
        assert_select '.asset-logo'
        assert_select 'b', text: 'BTC'
        assert_select '.pill--quiet', text: 'Crypto'
        assert_select '.slider__style__track'
        assert_select '.tracker-holdings__pct', text: '75.0%'
        assert_select '.tracker-holdings__value', text: /\$60,000\.00/
        assert_select '.tracker-holdings__pl.text-success', text: /\+99\.9%/
      end
      # ETH: the exchange holds 10 the history knows nothing about — taken as arrived at its price,
      # so nothing riding on it yet, and a note saying so.
      assert_select '.tracker-holdings__row:nth-child(2) .tracker-holdings__pl', text: '+0.0%'
      assert_select '.tracker-holdings__note', text: /Kraken.*10.*ETH/m
      assert_select 'details.tracker-holdings__more', false
    end
    assert_select '[data-controller="donut-chart"]', false, 'the tilted donut and its pie/list toggle are gone'
  end

  # An unlinked deposit has no fill price, so its basis is the day's market value — an estimate.
  # Stated, and said to be an estimate, rather than withheld.
  test 'a holding whose basis was assumed states its P/L, and says the cost is an estimate' do
    create(:account_transaction, api_key: @key_kraken, entry_type: :deposit, base_currency: 'ETH',
                                 base_amount: 10, quote_currency: nil, quote_amount: nil,
                                 transacted_at: @t - 5.days)
    HistoricalPrice.create!(asset: 'ETH', currency: 'USD', date: (@t - 5.days).to_date, price: 1_800)
    warm_ledger
    get tracker_path

    assert_select '.tracker-holdings__row:nth-child(2) b', text: 'ETH'
    assert_select '.tracker-holdings__row:nth-child(2) .tracker-holdings__pl', text: '+11.1%'
    assert_select '.tracker-holdings__note', text: /ETH.*market price/
  end

  test 'more than six holdings fold behind a More disclosure' do
    warm_ledger
    7.times { |i| balance(@binance, create(:asset, symbol: "C#{i}", external_id: "c#{i}"), free: 1, price: 10) }
    get tracker_path

    assert_select '.tracker-holdings__rows > .tracker-holdings__row', 6
    assert_select 'details.tracker-holdings__more summary', text: /#{I18n.t('tracker.show_more')}/
    assert_select 'details.tracker-holdings__more .tracker-holdings__row', 3
  end

  # ── 5 · record bar ───────────────────────────────────────────────────────────────────────────
  test 'the record bar: the pane switch, contextual filters in their own order-filter scopes, range, Export, Tax report' do
    warm_ledger
    get tracker_path

    assert_select '.tracker-record .filters > .segmented .segmented__option[data-value="tx"]'
    assert_select '.tracker-record .filters > .segmented .segmented__option[data-value="pos"]'
    assert_select '.tracker-record [data-controller~="order-filter"]', 2
    # The types this account actually holds, in the ledger's own order — not a fixed five. A
    # transfer option appears only once a linked pair is on the page.
    %w[all buy sell deposit withdrawal].each do |type|
      assert_select ".tracker-record [data-controller~='order-filter'] .segmented__option[data-value='#{type}']"
    end
    assert_select ".tracker-record .segmented__option[data-value='swap_in']", false, 'nothing swapped here'
    assert_select ".tracker-record .segmented__option[data-value='transfer']", false, 'nothing is linked here'
    # One open position and one closed win: Loss is not offered, because there is none.
    %w[all open win].each do |status|
      assert_select ".tracker-record [data-controller~='order-filter'] .segmented__option[data-value='#{status}']"
    end
    assert_select ".tracker-record .segmented__option[data-value='loss']", false, 'nothing lost here'
    assert_select '.tracker-record input[type=date][name=from]'
    assert_select '.tracker-record input[type=date][name=to]'
    assert_select '.tracker-record a.rbutton', false, 'what the page hands you lives on the scope line now'
    assert_select '.sbutton--sky', false, 'no filled primary button in the bar — the bot view uses rbuttons here'
  end

  test 'the switch scopes the chart and the tiles, not just the card below them' do
    Tracker::Ledger.compute!(@user, exchange: @binance)
    PortfolioSnapshot::BackfillJob.perform_now(@user.id, @binance.id)

    get tracker_path(exchange_id: @binance.id)

    invested = JSON.parse(css_select('[data-controller="bot--chart"]').first['data-bot--chart-series-value'])[1]
    assert_equal 30_020.0, invested.last, 'the invested curve is what went into THIS venue'
    assert_select '.data-grid__item__value', { text: /\$60,000\.00/, count: 1 },
                  'and the value tile is this venue\'s balances, not the portfolio\'s'
    assert_select '.data-grid__item__value', { text: /\$30,020\.00/, count: 1 },
                  'as is the money-in tile'
  end

  test 'a cold scoped history warms itself once' do
    PortfolioSnapshot::BackfillJob.expects(:perform_later).with(@user.id, @binance.id).once

    get tracker_path(exchange_id: @binance.id)

    assert_response :success
    assert_select '.widget--chart .widget--chart__plot .loader', true, 'it spins while it warms'
  end

  test 'a holding bought since the balances were taken keeps its P/L' do
    AccountBalance.for_user(@user).update_all(synced_at: 2.hours.ago)
    create(:account_transaction, api_key: @key_binance, entry_type: :buy, base_currency: 'BTC', base_amount: 0.5,
                                 quote_currency: 'USD', quote_amount: 20_000, transacted_at: 1.hour.ago)
    warm_ledger

    get tracker_path

    assert_select '.tracker-holdings__row .tracker-holdings__pl.text-success', 2,
                  'the ledger is ahead of the balance by exactly what it bought since — that is not a hole; the ETH arrived at its price'
  end

  test 'a holding the history overstates is priced at what is held, and says what left' do
    create(:account_transaction, api_key: @key_binance, entry_type: :buy, base_currency: 'BTC', base_amount: 0.5,
                                 quote_currency: 'USD', quote_amount: 20_000, transacted_at: @t - 20.days)
    warm_ledger

    get tracker_path

    assert_select '.tracker-holdings__row:first-child .tracker-holdings__pl[title]', 0, 'stated, on an assumption'
    assert_select '.tracker-holdings__note', text: /Binance.*1\.5.*2\.00/m, count: 1
  end

  test 'the chart shows a spinner while its history is being built, and no chart at all without one' do
    PortfolioSnapshot::BackfillJob.stubs(:perform_later)

    get tracker_path(exchange_id: @binance.id)

    assert_select '.widget--chart .loader', 1, 'a scope whose series is still being swept'
    assert_select '.widget--chart canvas', false, 'and no empty axis pretending to be a chart'
  end

  test 'the folded holdings open from a Show more that becomes a Show less' do
    %w[SOL ADA DOT LINK XRP ATOM].each_with_index do |symbol, index|
      balance(@binance, create(:asset, symbol: symbol, name: symbol), free: 1, price: 100 - index)
    end

    get tracker_path

    assert_select '.tracker-holdings__more summary', text: /#{I18n.t('tracker.show_more')}/
    assert_select '.tracker-holdings__more summary', text: /#{I18n.t('tracker.show_less')}/
  end

  test 'the scope line opens with the venues and ends with what the page can hand you' do
    get tracker_path

    assert_select '.tracker-exchanges > .filters:first-child', true, 'the scope switch opens the line'
    # `bot.details.stats.download_csv`, not `bot.details.download_csv`: the plan named the shorter
    # path, but the key the bot page has actually carried in all 15 locales since it shipped is the
    # one under `stats`. Reusing it beats adding a fifteen-file duplicate that says "Export" twice.
    assert_select ".tracker-exchanges__end a.rbutton[href^='#{export_tracker_path}']",
                  text: I18n.t('bot.details.stats.download_csv')
    assert_select ".tracker-exchanges__end a.rbutton[href='#{export_modal_tracker_path}']",
                  text: I18n.t('tracker.get_report')
    assert_select '.tracker-exchanges > *:last-child.tracker-exchanges__end', true, 'and they close it'
  end

  # ── 6 · transactions table ───────────────────────────────────────────────────────────────────
  test 'transaction rows: type pill, asset with logo, price column, fee in its currency, bot number, hover transfer action' do
    warm_ledger
    bot = create(:dca_single_asset, user: @user, exchange: @binance, base_asset: @btc)
    fill = create(:transaction, bot: bot)
    create(:account_transaction, api_key: @key_binance, entry_type: :buy, base_currency: 'BTC', base_amount: 0.01,
                                 quote_currency: 'USD', quote_amount: 500, transacted_at: @t, bot_transaction: fill)
    get tracker_path

    assert_select 'th', text: /\A#{I18n.t('tracker.columns.price')}/
    assert_select 'tr.tracker-row[data-order-filter-target="row"][data-order-type~="buy"][data-order-type~="all"]' do
      # One chip vocabulary across the page: the tone says what happened, the text says which.
      assert_select '.pill.pill--up', text: I18n.t('tracker.types.buy')
      assert_select 'td .asset-logo'
    end
    assert_select "tr.tracker-row a[href='#{bot_path(bot)}']", text: "##{bot.id}"
    assert_select 'tr.tracker-row td', text: /50,000\.00/ # 500 / 0.01
    assert_select 'tr.tracker-row td', text: /20\.00 USD/ # the fee
    transfer_action = toggle_transfer_tracker_transaction_path(@withdrawal)
    assert_select "tr.tracker-row[data-order-type~='withdrawal'] form.tracker-row__action[action='#{transfer_action}']"
    assert_select 'tr.tracker-row .sinput--small', false, 'the inline Mark-as-transfer button is gone'
  end

  # ── 7 · positions table ──────────────────────────────────────────────────────────────────────
  test 'positions: open rows from the lots and closed round-trips with status pills' do
    warm_ledger
    get tracker_path

    # Three: the BTC still held, the ETH round-trip that closed, and the ETH still held after it —
    # the table lists what is HELD, so a holding cannot appear on the card above and be missing here.
    assert_select '.tracker-positions tbody tr', 3
    assert_select '.tracker-positions tr[data-order-type~="open"]' do
      assert_select 'td', text: /BTC/
      assert_select '.pill.pill--info', text: I18n.t('tracker.record.open')
    end
    assert_select '.tracker-positions tr[data-order-type~="win"]' do
      assert_select 'td', text: /ETH/
      assert_select '.pill.pill--up', text: I18n.t('tracker.record.win')
      assert_select 'td.text-success', text: /\+25\.0%/
    end
    assert_select 'th', text: I18n.t('tracker.columns.avg_buy')
    assert_select 'th', text: I18n.t('tracker.columns.hold')
  end

  test 'positions is the pane the page opens on' do
    warm_ledger
    get tracker_path

    assert_select ".tracker-record__pane[data-pane='pos']:not(.tracker-record__pane--off)"
    assert_select ".tracker-record__pane[data-pane='tx'].tracker-record__pane--off"
    assert_select '.tracker-record__switch .segmented__option.is-on', text: I18n.t('tracker.positions')
  end

  # ── 8 · exchange scope ───────────────────────────────────────────────────────────────────────
  test 'exchange_id scopes the whole view: holdings, transactions, positions and the figures' do
    warm_ledger
    Tracker::Ledger.compute!(@user, exchange: @binance)
    get tracker_path(exchange_id: @binance.id)

    assert_select '.tracker-exchanges a.segmented__option[aria-current="page"]', text: 'Binance'
    assert_select '.tracker-holdings__row', 1
    assert_select '.tracker-holdings__row b', text: 'BTC'
    assert_select 'tr.tracker-row td', text: 'ETH', count: 0
    assert_select '.tracker-positions tbody tr', 1
    assert_select '.data-grid__item__value', { text: /\$60,000\.00/, count: 1 },
                  'this venue holds 1.5 BTC at 40k; the ETH at the other one is not this page'
    assert_select '.data-grid__item__value', { text: /\$30,020\.00/, count: 1 },
                  'and 30,020 went into it, not the 34,020 the portfolio has taken in'
    assert_select '.data-grid__item__value', text: /\$80,000\.00/, count: 0
  end

  test 'a cold exchange-scoped ledger warms its own key' do
    warm_ledger
    Tracker::LedgerJob.expects(:perform_later).with(@user.id, @binance.id).once
    get tracker_path(exchange_id: @binance.id)

    assert_select '.tracker-positions .loader--small'
  end

  # ── 9 · hide balances ────────────────────────────────────────────────────────────────────────
  test 'with balances hidden no money is rendered: no tiles, no values, percentages stay' do
    warm_ledger
    @user.update!(hide_balances: true)
    get tracker_path

    assert_select '.data-grid', false
    assert_select '.tracker-holdings__value', false
    assert_select '.tracker-holdings__pct', text: '75.0%'
    assert_select '.tracker-record tr.tracker-row td', text: /\$/, count: 0
    assert_select '.tracker-positions td', text: /\$/, count: 0
    assert_select '.tracker-positions td.text-success', text: /\+25\.0%/, count: 1
    # The figures this page would otherwise print, in every form they could take.
    assert_no_match(/80,000|60,000|34,020|30,020|20,013|20\.00 USD|4,000\.00|5,000\.00|1,000\.00/, response.body)
  end

  # ── 10 · display currency ────────────────────────────────────────────────────────────────────
  test 'figures follow the display currency' do
    warm_ledger
    Utilities::Currency.stubs(:exchange_rate).with(from: 'USD', to: 'PLN').returns(Result::Success.new(4.0))
    @user.update!(display_currency: 'PLN')
    get tracker_path

    assert_select '.data-grid__item__value', text: /320,000\.00 zł/
    assert_select '.tracker-holdings__value', text: /240,000\.00 zł/
  end
end
