# frozen_string_literal: true

require 'test_helper'

# A multi-asset bot becomes one single-asset bot per member. What matters is what each child is made of
# (the source's schedule, its share of the amount), where every row goes and as what, that the children's
# books add up to the source's, and what the split refuses — above all, money from past sales nobody has
# answered for.
class Bot::SplitTest < ActiveSupport::TestCase
  setup do
    Bot::UpdateMetricsJob.stubs(:perform_later)
    @user = create(:user)
    @exchange = create(:binance_exchange)
    @usd = create(:asset, :usd)
    @btc = create(:asset, :bitcoin)
    @eth = create(:asset, :ethereum)
    @sol = create(:asset, symbol: 'SOL', name: 'Solana', external_id: 'solana')
  end

  # == What the split produces ==

  test 'each member becomes a stopped one-asset basket spending its share of the schedule' do
    source = basket([@btc, @eth], allocations: { @btc => 0.6, @eth => 0.4 })
    write_settings!(source, interval: 'week', quote_amount: 100)

    children = split!(source)

    assert_equal %w[Bitcoin Ethereum], children.map(&:label)
    children.each do |child|
      assert_instance_of Bots::DcaMultiAsset, child
      assert_predicate child, :stopped?
      assert_equal @exchange, child.exchange
      assert_equal 'week', child.interval
      assert_equal source.position, child.position
      assert_predicate child, :one_asset?
    end
    assert_in_delta 60, children.first.quote_amount.to_f, 1e-9
    assert_in_delta 40, children.last.quote_amount.to_f, 1e-9
    assert_predicate source.reload, :deleted?
  end

  test 'a member that no longer trades gets no bot, and the others keep the whole schedule' do
    source = basket([@btc, @eth, @sol], allocations: { @btc => 0.5, @eth => 0.3, @sol => 0.2 })
    write_settings!(source, quote_amount: 100)
    ticker_for(@sol).update_columns(trading_enabled: false)

    children = split!(source)

    assert_equal %w[Bitcoin Ethereum], children.map(&:label)
    assert_in_delta 100, children.sum { |child| child.quote_amount.to_f }, 1e-9
    assert_in_delta 62.5, children.first.quote_amount.to_f, 1e-9
  end

  test 'every row moves to the bot of its asset as REGULAR; the rest go to the first bot, exited' do
    source = basket([@btc, @eth], allocations: { @btc => 0.6, @eth => 0.4 })
    membership!(source, @sol, 0, in_index: false, exited_at: 1.day.ago)
    btc = order!(source, base: 'BTC')
    eth = order!(source, base: 'ETH', transaction_type: 'REBALANCE')
    sol = order!(source, base: 'SOL')
    sold = order!(source, base: 'SOL', side: :sell, transaction_type: 'LIQUIDATION', price: 100)
    order!(source, base: 'BTC', transaction_type: 'REDEPLOY', quote_amount: 100, quote_amount_exec: 100)

    btc_bot, eth_bot = split!(source)

    assert_equal btc_bot.id, btc.reload.bot_id
    assert_equal eth_bot.id, eth.reload.bot_id
    assert_equal [btc_bot.id], [sol.reload.bot_id, sold.reload.bot_id].uniq
    assert_equal %w[REGULAR], Transaction.where(bot_id: [btc_bot.id, eth_bot.id]).distinct.pluck(:transaction_type)
    exited = btc_bot.bot_index_assets.find_by(asset: @sol)
    assert_not exited.in_index
    assert_equal [@btc.id], btc_bot.bot_index_assets.in_index.pluck(:asset_id)
    assert_equal 0, btc_bot.redeploy_offer(btc_bot.metrics(force: true)), 'history offers nothing again'
    assert_equal btc_bot.transactions.maximum(:id), btc_bot.transient_data['merged_history_until_id']
  end

  test 'a legacy row with no recorded asset goes to the bot of its symbol, ours or the venue\'s' do
    source = basket([@btc, @eth])
    legacy = order!(source, base: 'ETH', base_asset_id: nil, resolve_asset_ids: false)

    ticker_for(@btc).update_columns(base: 'XBT')
    venue_spelled = order!(source, base: 'XBT', base_asset_id: nil, resolve_asset_ids: false)

    btc_bot, eth_bot = split!(source)

    assert_nil legacy.reload.base_asset_id
    assert_equal eth_bot.id, legacy.bot_id
    assert_equal btc_bot.id, venue_spelled.reload.bot_id
  end

  # == The books ==

  test 'a swap splits into a sale and a purchase: the children together made what the source made' do
    source = basket([@btc, @eth])
    t = 3.days.ago
    order!(source, base: 'BTC', created_at: t)
    order!(source, base: 'BTC', side: :sell, transaction_type: 'REBALANCE', created_at: t + 1.hour,
                   price: 120, amount: 0.5, amount_exec: 0.5, quote_amount: 60, quote_amount_exec: 60)
    order!(source, base: 'ETH', transaction_type: 'REBALANCE', created_at: t + 2.hours,
                   price: 60, quote_amount: 60, quote_amount_exec: 60)
    before = source.metrics(force: true)

    children = split!(source)
    after = children.map { |child| child.metrics(force: true) }

    assert_equal(profit(before), after.sum { |data| profit(data) })
    btc_books, eth_books = after
    assert_equal 60, eth_books[:total_quote_amount_invested].to_d, 'the swap buy is new money for ETH'
    assert_equal 60, btc_books[:rebalance_cash].to_d, 'the swap sale is money out for BTC'
    assert_equal(0.5, btc_books[:asset_breakdown].values.sum { |entry| entry[:amount].to_d })
  end

  test 'a liquidation and its redeploy split into a sale and a purchase, P/L kept' do
    source = basket([@btc, @eth])
    membership!(source, @sol, 0, in_index: false, exited_at: 1.day.ago)
    t = 3.days.ago
    order!(source, base: 'SOL', created_at: t)
    order!(source, base: 'BTC', created_at: t + 1.minute)
    order!(source, base: 'SOL', side: :sell, transaction_type: 'LIQUIDATION', created_at: t + 1.hour,
                   price: 150, quote_amount: 150, quote_amount_exec: 150)
    order!(source, base: 'ETH', transaction_type: 'REDEPLOY', created_at: t + 2.hours,
                   price: 150, quote_amount: 150, quote_amount_exec: 150)
    before = source.metrics(force: true)
    assert_equal 0, source.redeploy_offer(before), 'already redeployed'

    children = split!(source)

    assert_equal(profit(before), children.sum { |child| profit(child.metrics(force: true)) })
  end

  test 'an overspent swap buy is new money on the child, where the source never booked it' do
    source = basket([@btc, @eth])
    t = 3.days.ago
    order!(source, base: 'BTC', created_at: t)
    order!(source, base: 'BTC', side: :sell, transaction_type: 'REBALANCE', created_at: t + 1.hour,
                   price: 120, amount: 0.5, amount_exec: 0.5, quote_amount: 60, quote_amount_exec: 60)
    order!(source, base: 'ETH', transaction_type: 'REBALANCE', created_at: t + 2.hours,
                   price: 61, quote_amount: 61, quote_amount_exec: 61)
    before = source.metrics(force: true)

    children = split!(source)

    assert_equal(profit(before) - 1, children.sum { |child| profit(child.metrics(force: true)) })
  end

  test 'a child merged later keeps the books it was split with' do
    source = basket([@btc, @eth])
    t = 3.days.ago
    order!(source, base: 'BTC', created_at: t)
    order!(source, base: 'BTC', side: :sell, transaction_type: 'REBALANCE', created_at: t + 1.hour,
                   price: 120, amount: 0.5, amount_exec: 0.5, quote_amount: 60, quote_amount_exec: 60)
    order!(source, base: 'ETH', transaction_type: 'REBALANCE', created_at: t + 2.hours,
                   price: 60, quote_amount: 60, quote_amount_exec: 60)
    _btc_bot, eth_bot = split!(source)
    other = basket([@sol])
    order!(other, base: 'SOL', created_at: t + 3.hours)
    expected = profit(eth_bot.metrics(force: true)) + profit(other.metrics(force: true))

    merge = Bot::Merge.new(@user, [eth_bot.id, other.id])
    merged = merge.perform!

    assert merged, merge.error
    assert_equal expected, profit(merged.metrics(force: true))
  end

  # == Money from past sales ==

  test 'proceeds nobody answered for refuse the split; kept, they go through' do
    source = with_proceeds(basket([@btc, @eth]))

    split = Bot::Split.new(@user, [source.id])
    assert_nil split.reason, 'proceeds are a question in the modal, not a refusal'
    assert_equal 150, split.offers[source].to_d
    assert_nil split.perform!
    assert_equal I18n.t('errors.bots.split.proceeds', label: source.label), split.error
    assert_not_predicate source.reload, :deleted?

    assert Bot::Split.new(@user, [source.id], keep_ids: [source.id]).perform!
    assert_predicate source.reload, :deleted?
  end

  test 'a reinvestment still running refuses until it settles, and the bot stays pickable in the modal' do
    source = basket([@btc, @eth])
    order!(source, base: 'BTC', transaction_type: 'REDEPLOY', status: :submitted, external_status: :open,
                   amount_exec: nil, quote_amount_exec: nil)

    split = Bot::Split.new(@user, [source.id])

    assert_nil split.reason
    assert split.reinvesting?(source)
    assert_not Bot::Split.splittable?(source), 'the tile waits'
    assert_nil split.perform!
    assert_equal I18n.t('errors.bots.split.reinvesting', label: source.label), split.error
  end

  test 'a Yes still queued reads as reinvesting' do
    source = basket([@btc, @eth])
    assert_not Bot::Split.new(@user, [source.id]).reinvesting?(source)

    # Ready, as perform_later leaves it until a worker claims it.
    SolidQueue::Job.create!(queue_name: 'default', class_name: 'Bot::RedeployJob', priority: 0,
                            arguments: { 'job_class' => 'Bot::RedeployJob',
                                         'arguments' => [{ '_aj_globalid' => source.to_global_id.to_s }] })

    assert Bot::Split.new(@user, [source.id]).reinvesting?(source)
  end

  # == Refusals ==

  test 'a one-asset bot, a stranger bot and nothing at all are refused' do
    single = basket([@btc])
    stranger = create(:dca_multi_asset, user: create(:user), exchange: @exchange, quote_asset: @usd,
                                        base_assets: [@btc, @eth], status: :stopped)

    assert_equal [:unavailable, { label: single.label }], Bot::Split.new(@user, [single.id]).reason
    assert_equal :missing, Bot::Split.new(@user, [stranger.id]).reason
    assert_equal :none, Bot::Split.new(@user, []).reason
  end

  test 'a bot that sold units it never bought is refused' do
    source = basket([@btc, @eth])
    order!(source, base: 'BTC', side: :sell, amount: 2, amount_exec: 2, quote_amount: 200, quote_amount_exec: 200)

    assert_equal [:external_sales, { label: source.label }], Bot::Split.new(@user, [source.id]).reason
  end

  test 'a bot with a sale the venue has not priced is refused' do
    source = basket([@btc, @eth])
    order!(source, base: 'BTC', created_at: 2.days.ago)
    order!(source, base: 'BTC', side: :sell, transaction_type: 'LIQUIDATION', quote_amount_exec: nil)

    assert_equal [:unpriced_sales, { label: source.label }], Bot::Split.new(@user, [source.id]).reason
  end

  test 'a held venue lease refuses and changes nothing' do
    source = basket([@btc, @eth])
    order!(source, base: 'BTC')
    key = Bot::VenueLease::ExchangeLease.for(@exchange).concurrency_key
    SolidQueue::Semaphore.create!(key:, value: 0, expires_at: 5.minutes.from_now)

    assert_nil Bot::Split.new(@user, [source.id]).perform!
    assert_not_predicate source.reload, :deleted?
    assert_equal [source.id], Transaction.distinct.pluck(:bot_id)
  end

  test 'several bots split in one go' do
    one = basket([@btc, @eth])
    two = basket([@eth, @sol])

    children = Bot::Split.new(@user, [one.id, two.id]).perform!

    assert_equal %w[Bitcoin Ethereum Ethereum Solana], children.map(&:label)
    assert_equal [@btc, @eth, @sol], Bot::Split.new(@user, [basket([@btc, @eth]).id, basket([@eth, @sol]).id]).assets
  end

  private

  def split!(source)
    split = Bot::Split.new(@user, [source.id])
    children = split.perform!
    assert children, split.error
    children
  end

  # What the bot made, in money: value less what went in — not the return ratio.
  def profit(data)
    data[:total_amount_value_in_quote].to_d - data[:total_quote_amount_invested].to_d
  end

  # 150 of liquidation proceeds, not yet answered.
  def with_proceeds(bot)
    membership!(bot, @sol, 0, in_index: false, exited_at: 1.day.ago)
    t = 3.days.ago
    order!(bot, base: 'SOL', created_at: t)
    order!(bot, base: 'SOL', side: :sell, transaction_type: 'LIQUIDATION', created_at: t + 1.hour,
                price: 150, quote_amount: 150, quote_amount_exec: 150)
    bot
  end

  def basket(assets, allocations: nil, **attrs)
    create(:dca_multi_asset, user: @user, exchange: @exchange, quote_asset: @usd, base_assets: assets,
                             allocations: allocations, status: :stopped, **attrs)
  end

  def ticker_for(asset)
    Ticker.find_by(exchange: @exchange, base_asset: asset, quote_asset: @usd) ||
      create(:ticker, exchange: @exchange, base_asset: asset, quote_asset: @usd)
  end

  def membership!(bot, asset, weight, in_index: true, entered_at: 1.week.ago, exited_at: nil)
    BotIndexAsset.create!(bot:, asset:, ticker: ticker_for(asset), in_index:, target_allocation: weight,
                          entered_at:, exited_at:)
  end

  def write_settings!(bot, **pairs)
    bot.update_columns(settings: bot.settings.merge(pairs.stringify_keys))
    bot.reload
  end

  def order!(bot, base:, **columns)
    create(:transaction, exchange: @exchange, bot:, base:, quote: 'USD', status: :submitted, external_status: :closed,
                         side: :buy, external_id: "o-#{SecureRandom.hex(4)}", price: 100, amount: 1, amount_exec: 1,
                         quote_amount: 100, quote_amount_exec: 100, **columns)
  end
end
