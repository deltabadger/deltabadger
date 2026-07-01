require 'test_helper'

# M4 — Locked PnL. Sells realize cash: value = realized_proceeds + net_base * price. Once profit
# is realized by selling, the cash proceeds stop floating with price, so the green PnL line locks.
class Bots::DcaSingleAsset::LockedPnlTest < ActiveSupport::TestCase
  include ActiveSupport::Testing::TimeHelpers

  def buy(bot, price:, amount:, at:)
    create(:transaction, bot: bot, side: :buy, price: price, amount_exec: amount,
                         quote_amount_exec: price * amount, created_at: at)
  end

  def sell(bot, price:, amount:, at:)
    create(:transaction, bot: bot, side: :sell, price: price, amount_exec: amount,
                         quote_amount_exec: price * amount, created_at: at)
  end

  # The plan's worked example: invest $100 → 1 BTC@100; BTC→$200 ⇒ +100%; sell 1 BTC@200 ⇒ still
  # +100%; BTC→$50 ⇒ +100% LOCKED (vs −50% if never sold).
  test 'realizing profit by selling locks the PnL in the static metrics' do
    bot = create(:dca_single_asset, :started)
    buy(bot, price: 100, amount: 1, at: 3.days.ago)
    sell(bot, price: 200, amount: 1, at: 2.days.ago)

    m = bot.metrics(force: true)

    assert_in_delta 0,   m[:total_base_amount].to_f, 1e-9           # net base back to 0
    assert_in_delta 200, m[:total_realized_proceeds].to_f, 1e-9     # cash locked in
    assert_in_delta 100, m[:total_quote_amount_invested].to_f, 1e-9 # invested = cumulative buys
    assert_in_delta 200, m[:total_amount_value_in_quote].to_f, 1e-9 # 200 + 0*price
    assert_in_delta 1.0, m[:pnl], 1e-9                              # +100%
  end

  test 'locked PnL holds even when the price crashes after selling' do
    bot = create(:dca_single_asset, :started)
    buy(bot, price: 100, amount: 1, at: 3.days.ago)
    sell(bot, price: 200, amount: 1, at: 2.days.ago)
    bot.exchange.stubs(:get_tickers_prices).returns(Result::Success.new({ bot.ticker.ticker => 50.to_d }))

    m = bot.metrics_with_current_prices(force: true)

    assert_in_delta 200, m[:total_amount_value_in_quote].to_f, 1e-9 # 200 realized + 0*50
    assert_in_delta 1.0, m[:pnl], 1e-9                              # +100% locked, NOT −50%
  end

  test 'an unrealized position still floats with the price (no sell yet)' do
    bot = create(:dca_single_asset, :started)
    buy(bot, price: 100, amount: 1, at: 3.days.ago)
    bot.exchange.stubs(:get_tickers_prices).returns(Result::Success.new({ bot.ticker.ticker => 50.to_d }))

    m = bot.metrics_with_current_prices(force: true)

    assert_in_delta 50, m[:total_amount_value_in_quote].to_f, 1e-9 # 0 realized + 1*50
    assert_in_delta(-0.5, m[:pnl], 1e-9)                           # −50% (floats, not locked)
  end

  test 'the value series locks realized proceeds at each chart point' do
    bot = create(:dca_single_asset, :started)
    buy(bot, price: 100, amount: 1, at: 3.days.ago)
    sell(bot, price: 200, amount: 1, at: 2.days.ago)

    m = bot.metrics(force: true)

    # point 1 (after buy): 0 + 1*100; point 2 (after sell): 200 + 0*200
    assert_in_delta 100, m[:chart][:series][0][0].to_f, 1e-9
    assert_in_delta 200, m[:chart][:series][0][1].to_f, 1e-9
    # extra_series[0] = net_base, extra_series[1] = realized_proceeds (for candle interpolation)
    assert_in_delta 1, m[:chart][:extra_series][0][0].to_f, 1e-9
    assert_in_delta 0, m[:chart][:extra_series][0][1].to_f, 1e-9
    assert_in_delta 0,   m[:chart][:extra_series][1][0].to_f, 1e-9
    assert_in_delta 200, m[:chart][:extra_series][1][1].to_f, 1e-9
  end

  test 'candle interpolation recomputes value = realized_proceeds + net_base * candle_open' do
    bot = create(:dca_single_asset, :started)
    buy(bot, price: 100, amount: 1, at: 3.days.ago)
    sell(bot, price: 200, amount: 1, at: 2.days.ago)
    bot.metrics(force: true)
    # a candle after the sell: net_base 0, realized 200 → value 200 regardless of the open price
    CandleSeriesCache.stubs(:fetch).returns(Result::Success.new([[1.day.ago, 50.to_d]]))

    result = bot.send(:get_extended_chart_data_with_candles_data)

    assert_predicate result, :success?
    assert_in_delta 200, result.data[:series][0].last.to_f, 1e-9
  end

  test 'average_buy_price weights buys only (a sell does not skew it)' do
    bot = create(:dca_single_asset, :started)
    buy(bot, price: 100, amount: 1, at: 4.days.ago)
    buy(bot, price: 200, amount: 1, at: 3.days.ago)
    sell(bot, price: 1000, amount: 1, at: 2.days.ago) # extreme sell price must NOT enter the average

    m = bot.metrics(force: true)

    assert_in_delta 150, m[:average_buy_price].to_f, 1e-9 # (100 + 200) / 2
  end

  test 'net base never goes negative defensively' do
    bot = create(:dca_single_asset, :started)
    buy(bot, price: 100, amount: 0.2, at: 3.days.ago)
    sell(bot, price: 100, amount: 0.5, at: 2.days.ago) # oversell (shouldn't happen; clamp anyway)

    m = bot.metrics(force: true)

    assert_operator m[:total_base_amount].to_f, :>=, 0
  end

  # == only CONFIRMED (closed) executions are realized ==
  # The "null exec == filled for the requested amount" fallback is valid only for closed rows. An
  # accepted-but-unfilled order must not be realized, or an open limit sell would show cash locked in
  # and base sold before any fill is confirmed.

  test 'an open (unfilled) sell does not realize proceeds or reduce holdings' do
    bot = create(:dca_single_asset, :started)
    buy(bot, price: 100, amount: 1, at: 3.days.ago)
    create(:transaction, bot: bot, side: :sell, external_status: :open, external_id: 'open-sell',
                         price: 200, amount: 1, amount_exec: nil, quote_amount_exec: nil, created_at: 2.days.ago)

    m = bot.metrics(force: true)

    assert_in_delta 1,   m[:total_base_amount].to_f, 1e-9        # still holding the 1 BTC
    assert_in_delta 0,   m[:total_realized_proceeds].to_f, 1e-9  # nothing realized until it fills
    assert_in_delta 100, m[:total_quote_amount_invested].to_f, 1e-9
  end

  test 'an open (unfilled) buy does not count toward holdings or invested' do
    bot = create(:dca_single_asset, :started)
    buy(bot, price: 100, amount: 1, at: 3.days.ago)
    create(:transaction, bot: bot, side: :buy, external_status: :open, external_id: 'open-buy',
                         price: 100, amount: 5, amount_exec: nil, quote_amount_exec: nil, created_at: 2.days.ago)

    m = bot.metrics(force: true)

    assert_in_delta 1,   m[:total_base_amount].to_f, 1e-9
    assert_in_delta 100, m[:total_quote_amount_invested].to_f, 1e-9
  end

  # A confirmed (closed) legacy fill that never backfilled exec amounts still counts via the fallback.
  test 'a closed legacy row missing exec amounts is still realized via the fallback' do
    bot = create(:dca_single_asset, :started)
    create(:transaction, bot: bot, side: :buy, external_status: :closed, external_id: 'legacy',
                         price: 100, amount: 1, amount_exec: nil, quote_amount_exec: nil, created_at: 3.days.ago)

    m = bot.metrics(force: true)

    assert_in_delta 1,   m[:total_base_amount].to_f, 1e-9
    assert_in_delta 100, m[:total_quote_amount_invested].to_f, 1e-9
  end

  # == Selling beyond accumulated holdings is PnL-neutral on the excess ==
  # Base sold beyond what the bot bought is treated as acquired at its own sale price (cost basis =
  # sale price), so it adds equally to proceeds and to invested → zero profit/loss on that portion.

  test 'selling more base than the bot bought nets zero PnL on the excess' do
    bot = create(:dca_single_asset, :started)
    buy(bot, price: 100, amount: 1, at: 3.days.ago)  # the bot really bought 1 @ 100
    sell(bot, price: 200, amount: 3, at: 2.days.ago) # liquidate 3 @ 200 (2 from the wallet)

    m = bot.metrics(force: true)

    assert_in_delta 0,   m[:total_base_amount].to_f, 1e-9
    assert_in_delta 600, m[:total_realized_proceeds].to_f, 1e-9     # all cash received
    assert_in_delta 500, m[:total_quote_amount_invested].to_f, 1e-9 # 100 real + 2*200 phantom
    assert_in_delta 600, m[:total_amount_value_in_quote].to_f, 1e-9
    assert_in_delta 0.2, m[:pnl], 1e-9                              # +20%: the $100 is only the bought BTC
    # the invested series steps up at the excess sell (chart coherence)
    assert_in_delta 100, m[:chart][:series][1][0].to_f, 1e-9
    assert_in_delta 500, m[:chart][:series][1][1].to_f, 1e-9
  end

  test 'a sell-only bot that never bought shows zero PnL (pure liquidation)' do
    bot = create(:dca_single_asset, :started)
    sell(bot, price: 200, amount: 5, at: 2.days.ago) # never bought anything

    m = bot.metrics(force: true)

    assert_in_delta 0,    m[:total_base_amount].to_f, 1e-9
    assert_in_delta 1000, m[:total_realized_proceeds].to_f, 1e-9
    assert_in_delta 1000, m[:total_quote_amount_invested].to_f, 1e-9 # invested == proceeds
    assert_in_delta 0,    m[:pnl], 1e-9
  end

  test 'candle interpolation carries the stepped-up invested line after an excess sell' do
    bot = create(:dca_single_asset, :started)
    buy(bot, price: 100, amount: 1, at: 3.days.ago)
    sell(bot, price: 200, amount: 3, at: 2.days.ago)
    bot.metrics(force: true)
    CandleSeriesCache.stubs(:fetch).returns(Result::Success.new([[1.day.ago, 50.to_d]]))

    result = bot.send(:get_extended_chart_data_with_candles_data)

    assert_predicate result, :success?
    assert_in_delta 600, result.data[:series][0].last.to_f, 1e-9 # value: 600 realized + 0*50
    assert_in_delta 500, result.data[:series][1].last.to_f, 1e-9 # invested carries the stepped 500
  end
end
