require 'test_helper'

# The one place a restated count can still meet an un-restated price. A venue's "latest trade" is
# the last trade that actually happened, and a split is effective before the market opens — so
# between booking one and the first trade on the new basis, the live mark is the OLD price. Ten
# times the shares at ten times the price is ten times the money, and a rebalancer reading that
# sees an asset wildly overweight and sells it.
class Bot::RestatedPriceQuarantineTest < ActiveSupport::TestCase
  setup do
    @user = create(:user)
    @exchange = create(:alpaca_exchange)
    @api_key = create(:api_key, user: @user, exchange: @exchange)
    @usd = Asset.find_by(symbol: 'USD') || create(:asset, :usd)
    @klac = create(:asset, external_id: 'klac', symbol: 'KLAC')
    @aapl = create(:asset, external_id: 'aapl', symbol: 'AAPL')
    @bot = create(:dca_multi_asset, user: @user, exchange: @exchange, with_api_key: false,
                                    base_assets: [@klac, @aapl], quote_asset: @usd)
    create(:transaction, bot: @bot, exchange: @exchange, base: 'KLAC', quote: 'USD', side: :buy,
                         amount: 2, amount_exec: 2, price: 1000, quote_amount: 2000,
                         quote_amount_exec: 2000, created_at: 20.days.ago)
    Exchanges::Alpaca.any_instance.stubs(:get_tickers_prices)
                     .returns(Result::Success.new('KLACUSD' => 1000.to_d))
  end

  def split!(at:)
    create(:account_transaction, user: @user, api_key: @api_key, exchange: @exchange,
                                 entry_type: :adjustment, base_currency: 'KLAC', base_amount: 18,
                                 quote_currency: nil, quote_amount: nil, transacted_at: at,
                                 raw_data: { 'corporate_action' => 'split', 'split_ratio' => '10:1' })
  end

  test 'a fresh restatement puts the live prices out of trust' do
    split!(at: 2.hours.ago)

    assert @bot.restated_prices_untrusted?(@bot.metrics(force: true))
    assert @bot.metrics_with_current_prices(force: true)[:prices_stale]
  end

  test 'and the rebalancer will not size anything against them' do
    split!(at: 2.hours.ago)

    assert_nil @bot.send(:rebalance_targets)
  end

  test 'a restatement the market has long since priced is trusted again' do
    split!(at: 30.days.ago)

    assert_not @bot.restated_prices_untrusted?(@bot.metrics(force: true))
    assert_not @bot.metrics_with_current_prices(force: true)[:prices_stale]
  end

  test 'a bot with no restatement at all is never quarantined' do
    assert_not @bot.restated_prices_untrusted?(@bot.metrics(force: true))
    assert_not @bot.metrics_with_current_prices(force: true)[:prices_stale]
  end

  test 'the value shown during the quarantine is the restated one, not a tenfold one' do
    split!(at: 2.hours.ago)

    # The walk's own last-known price was restated with the position, so this holds 20 shares at
    # 100 rather than 20 at the 1000 the venue still reports.
    assert_equal 2000.to_d, @bot.metrics(force: true)[:total_amount_value_in_quote]
  end

  test 'a restatement that moved nothing is no reason to stand a bot down' do
    # Effective before the bot's first fill: the walk changes no holding, so the count and the
    # price were never on different bases.
    split!(at: 21.days.ago)

    assert_not @bot.restated_prices_untrusted?(@bot.metrics(force: true))
  end

  # A split this bot can see and cannot size never comes onto the same basis as the price, so this
  # one does not expire on a clock.
  test 'a split with no resolvable factor stands the bot down for as long as it stays unresolved' do
    split!(at: 60.days.ago)
    AccountTransaction.last.update!(raw_data: { 'corporate_action' => 'split' })

    assert_predicate @bot, :unresolved_split?
    assert @bot.restated_prices_untrusted?(@bot.metrics(force: true))
    assert_nil @bot.send(:rebalance_targets)
  end

  test 'the DCA leg stands down rather than sizing against the mismatch' do
    split!(at: 2.hours.ago)
    @bot.expects(:set_orders).never

    assert_predicate @bot.execute_action, :success?
    assert_equal 'dca_skipped_restatement', @bot.bot_activity_logs.last&.event
  end
end
