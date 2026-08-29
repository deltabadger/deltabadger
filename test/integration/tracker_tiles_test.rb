require 'test_helper'

# The figure grid. Six tiles that tell one story instead of three: what went in, what it is worth,
# what has been banked, what is still riding, what it cost to trade, and the one number those add up
# to. Before this the page stated a total on the chart, a realised figure in the grid and an
# unrealised PERCENTAGE in the donut — three scopes, two units, three places, and no way to check
# one against another.
class TrackerTilesTest < ActionDispatch::IntegrationTest
  setup do
    Tax::EcbFxRates.stubs(:ensure_loaded!)
    Rails.stubs(:cache).returns(ActiveSupport::Cache::MemoryStore.new)
    @user = create(:user, admin: true, setup_completed: true)
    @binance = create(:binance_exchange)
    @key = create(:api_key, user: @user, exchange: @binance)
    @btc = create(:asset, :bitcoin, color: '#F7931A')
    @t = Time.utc(2026, 8, 1, 12)
    # 1 BTC bought for 20,000, now worth 30,000: +10,000 unrealised.
    AccountBalance.create!(user: @user, exchange: @binance, asset: @btc, free: 1, locked: 0,
                           usd_price: 30_000, usd_value: 30_000,
                           synced_at: Time.current, priced_at: Time.current)
    create(:account_transaction, api_key: @key, exchange: @binance, entry_type: :deposit,
                                 base_currency: 'USD', base_amount: 20_000, quote_currency: nil,
                                 quote_amount: nil, transacted_at: @t - 10.days)
    create(:account_transaction, api_key: @key, exchange: @binance, entry_type: :buy,
                                 base_currency: 'BTC', base_amount: 1, quote_currency: 'USD',
                                 quote_amount: 20_000, transacted_at: @t - 9.days)
    Tracker::Ledger.compute!(@user)
    sign_in @user
  end

  def tiles
    get tracker_path
    css_select('.data-grid .data-grid__item').to_h do |item|
      [item.css('.label').text.strip.gsub(/\s+/, ' '), item.css('.data-grid__item__value').text.strip]
    end
  end

  # Facts first — what went in, what it is worth, what it cost — then the three outcomes.
  test 'the grid states all six figures, in that order' do
    labels = tiles.keys

    assert_equal([I18n.t('bot.details.stats.total_invested'), I18n.t('bot.details.stats.portfolio_value'),
                  I18n.t('tracker.fees_paid'), I18n.t('bot.dca_index.realised_pnl')],
                 labels.first(4).map { |label| label.sub(/\s*\?\z/, '') })

    %w[total_invested portfolio_value realised_pnl unrealised_pnl fees total_pnl].each do |key|
      wanted = I18n.t("tracker.tiles.#{key}", default: nil) || I18n.t("bot.details.stats.#{key}", default: nil)
      assert(labels.any? { |label| label.include?(wanted) }, "no tile for #{key} — got #{labels.inspect}") if wanted
    end
    assert_equal 6, labels.size
  end

  test 'the figures are the ones the ledger holds' do
    values = tiles.values.join(' ')

    assert_match '20,000', values, 'total invested'
    assert_match '30,000', values, 'portfolio value'
    assert_match '10,000', values, 'unrealised, and the total'
  end

  def money(figures, label) = figures.find { |k, _| k.include?(label) }&.last.to_s.gsub(/[^0-9.-]/, '').to_d

  # The formula, in the grid's own words: it names the two tiles at the head of the same row, so a
  # reader can check it by looking — and it is the figure the chart draws, so there is ONE total.
  test 'the total says it is the two tiles at the head of the row' do
    get tracker_path

    assert_select '.data-grid__item__hint'
    assert_select '.tooltip.tooltip--hint',
                  text: "#{I18n.t('bot.details.stats.portfolio_value')} − #{I18n.t('bot.details.stats.total_invested')}"
  end

  test 'the total is exactly portfolio value less total invested' do
    figures = tiles

    assert_equal money(figures, I18n.t('bot.details.stats.portfolio_value')) -
                 money(figures, I18n.t('bot.details.stats.total_invested')),
                 money(figures, I18n.t('tracker.tiles.total_pnl'))
  end

  # And with nothing in doubt, its two components come to it exactly — that is what makes them
  # components rather than three unrelated figures.
  test 'realised and unrealised come to the total when every holding is vouched for' do
    figures = tiles

    assert_select '.tracker-findings', false, 'nothing in doubt here'
    assert_equal money(figures, I18n.t('tracker.tiles.total_pnl')),
                 money(figures, I18n.t('bot.dca_index.realised_pnl')) +
                 money(figures, I18n.t('tracker.tiles.unrealised_pnl'))
  end

  # Half the BTC sold for 15,000 with a 100 fee: 4,900 banked, 5,000 still riding, 14,900 in cash.
  def sell_half!
    create(:account_transaction, api_key: @key, exchange: @binance, entry_type: :sell,
                                 base_currency: 'BTC', base_amount: 0.5, quote_currency: 'USD',
                                 quote_amount: 15_000, fee_amount: 100, fee_currency: 'USD',
                                 transacted_at: @t - 5.days)
    # The balances have to follow the sale — the coins that left AND the cash that arrived. Without
    # the cash the account genuinely does not add up, and the page now says so rather than showing
    # six figures that quietly contradict each other.
    AccountBalance.for_user(@user).sole.update!(free: 0.5, usd_value: 15_000)
    usd = create(:asset, symbol: 'USD', name: 'US Dollar')
    AccountBalance.create!(user: @user, exchange: @binance, asset: usd, free: 14_900, locked: 0,
                           usd_price: 1, usd_value: 14_900, synced_at: Time.current, priced_at: Time.current)
    Tracker::Ledger.compute!(@user)
  end

  # Fees are already off both halves — a disposal's gain is proceeds less cost less fee, and an
  # acquisition's fee is capitalised into the basis. Subtracting the Fees tile again would charge
  # them twice.
  test 'fees are not subtracted a second time' do
    sell_half!
    @user.update!(tracker_settings: { 'show_cash' => true })

    figures = tiles

    assert money(figures, I18n.t('tracker.fees_paid')).positive?, 'a fee was paid'
    assert_equal money(figures, I18n.t('tracker.tiles.total_pnl')),
                 money(figures, I18n.t('bot.dca_index.realised_pnl')) +
                 money(figures, I18n.t('tracker.tiles.unrealised_pnl')),
                 'the components still come to the total, with the fee already inside them'
  end

  # And with cash hidden they still come to it: the 14,900 the sale returned comes off the value
  # AND off the money in that funds it, so the difference between them — every outcome the grid
  # states — is exactly where it was.
  test 'hiding the cash moves both sides and no outcome' do
    sell_half!

    figures = tiles

    assert_equal 5_100.to_d, money(figures, I18n.t('bot.details.stats.total_invested')),
                 '20,000 in, less the 14,900 standing in cash'
    assert_equal 15_000.to_d, money(figures, I18n.t('bot.details.stats.portfolio_value'))
    assert_equal money(figures, I18n.t('tracker.tiles.total_pnl')),
                 money(figures, I18n.t('bot.dca_index.realised_pnl')) +
                 money(figures, I18n.t('tracker.tiles.unrealised_pnl'))
    assert_equal 9_900.to_d, money(figures, I18n.t('tracker.tiles.total_pnl'))
  end

  test 'hiding balances takes the whole grid, hint and all' do
    @user.update!(hide_balances: true)

    get tracker_path

    assert_select '.data-grid', false
  end
end
