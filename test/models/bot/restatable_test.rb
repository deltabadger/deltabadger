require 'test_helper'

# Where a bot learns that its share count was restated. Its own `transactions` table records orders
# and nothing else, so the only account of a corporate action is the broker's, in the ledger the
# account sync stores.
class Bot::RestatableTest < ActiveSupport::TestCase
  setup do
    @user = create(:user)
    @alpaca = create(:alpaca_exchange)
    @api_key = create(:api_key, user: @user, exchange: @alpaca)
    @usd = Asset.find_by(symbol: 'USD') || create(:asset, :usd)
  end

  def asset_for(symbol)
    Asset.find_by(symbol: symbol) || create(:asset, external_id: symbol.downcase, symbol: symbol)
  end

  def bot_trading(symbol, exchange: @alpaca)
    bot = create(:dca_single_asset, user: @user, exchange: exchange,
                                    base_asset: asset_for(symbol), quote_asset: @usd, with_api_key: false)
    trade(bot, symbol, exchange)
    bot
  end

  def trade(bot, symbol, exchange)
    base = asset_for(symbol)
    unless Ticker.exists?(exchange: exchange, base_asset: base, quote_asset: @usd)
      create(:ticker, exchange: exchange, base_asset: base, quote_asset: @usd)
    end
    create(:transaction, bot: bot, exchange: exchange, base: symbol, quote: 'USD')
  end

  def split_row(symbol: 'KLAC', ratio: '10:1', at: 3.days.ago, exchange: @alpaca, user: @user, **overrides)
    raw = { 'corporate_action' => 'split', 'split_ratio' => ratio }.compact
    api_key = user == @user ? @api_key : create(:api_key, user: user, exchange: exchange)
    create(:account_transaction, { user: user, api_key: api_key, exchange: exchange, entry_type: :adjustment,
                                   base_currency: symbol, base_amount: 90, quote_currency: nil,
                                   quote_amount: nil, transacted_at: at,
                                   raw_data: raw }.merge(overrides))
  end

  test 'a split of a symbol the bot traded on that venue becomes an event' do
    bot = bot_trading('KLAC')
    at = 3.days.ago
    split_row(at: at)

    assert_equal [[at, 'KLAC', 10.to_d]], bot.split_events
  end

  # The date is a de-duplication key, not the event time: `transacted_at` is written by parsing the
  # venue's date in `Time.zone`, which is per-request here, so rebuilding an instant from a UTC date
  # would move the split by a day for every zone ahead of UTC.
  test 'the event keeps the instant the venue booked, and the earliest when two report it' do
    bot = bot_trading('KLAC')
    early = Time.utc(2026, 6, 12, 4, 0, 0)
    split_row(at: Time.utc(2026, 6, 12, 21, 0, 0), tx_id: 'late')
    split_row(at: early, tx_id: 'early')

    assert_equal early, bot.split_events.sole.first
  end

  test 'a reverse split is a factor below one' do
    bot = bot_trading('KLAC')
    split_row(ratio: '1:8')

    assert_equal (1.to_d / 8), bot.split_events.sole.last
  end

  test 'a symbol the bot never traded is not its business' do
    bot = bot_trading('KLAC')
    split_row(symbol: 'AAPL')

    assert_empty bot.split_events
  end

  test 'a venue the bot never traded that symbol on is not its business' do
    kraken = create(:kraken_exchange)
    bot = bot_trading('KLAC')
    # The bot also traded something else on Kraken, so the venue is in its history — but not for
    # this symbol, and a cross-product would wrongly pull this in.
    trade(bot, 'BTC', kraken)
    split_row(symbol: 'KLAC', exchange: kraken)

    assert_empty bot.split_events
  end

  test 'a bot moved to another venue keeps the split of the venue it traded on' do
    bot = bot_trading('KLAC')
    split_row
    bot.update_columns(exchange_id: create(:kraken_exchange).id)

    assert_equal 1, bot.split_events.size
  end

  test "another user's restatement is not applied" do
    bot = bot_trading('KLAC')
    other = create(:user)
    split_row(user: other)

    assert_empty bot.split_events
  end

  test 'one action reported twice on the same date is one event' do
    bot = bot_trading('KLAC')
    split_row(at: Time.utc(2026, 6, 12, 4, 0, 0), tx_id: 'a')
    split_row(at: Time.utc(2026, 6, 12, 21, 0, 0), tx_id: 'b')

    assert_equal 1, bot.split_events.size
  end

  # Alpaca can ship the add leg alone and the merged pair a sync later; the sync keeps both on
  # purpose. The first says a split happened without saying by how much.
  test 'a row that names no factor does not veto one that does' do
    bot = bot_trading('KLAC')
    split_row(at: Time.utc(2026, 6, 12, 4, 0, 0), ratio: nil, tx_id: 'standalone')
    split_row(at: Time.utc(2026, 6, 12, 21, 0, 0), ratio: '10:1', tx_id: 'merged')

    assert_equal 10.to_d, bot.split_events.sole.last
  end

  test 'sources that disagree about the factor restate nothing' do
    bot = bot_trading('KLAC')
    split_row(at: Time.utc(2026, 6, 12, 4, 0, 0), ratio: '10:1', tx_id: 'a')
    split_row(at: Time.utc(2026, 6, 12, 21, 0, 0), ratio: '4:1', tx_id: 'b')

    assert_empty bot.split_events, 'a factor the sources disagree about must move no position'
  end

  test 'an adjustment with no provenance is not a split' do
    bot = bot_trading('KLAC')
    split_row(raw_data: { 'activity_type' => 'JNLC' })

    assert_empty bot.split_events
  end

  test 'a ratio that cannot be read fails closed' do
    bot = bot_trading('KLAC')
    ['', '10', '10:0', '0:10', '-10:1', 'ten:one', '10:1:2', nil].each_with_index do |ratio, i|
      split_row(ratio: ratio, at: (10 + i).days.ago, tx_id: "bad-#{i}")
    end

    assert_empty bot.split_events, 'nothing is restated on a factor that could not be parsed'
  end

  test 'events arrive oldest first' do
    bot = bot_trading('KLAC')
    split_row(at: 2.days.ago, tx_id: 'later')
    split_row(at: 9.days.ago, ratio: '2:1', tx_id: 'earlier')

    assert_equal [2.to_d, 10.to_d], bot.split_events.map(&:last)
  end

  test 'an action dated ahead of today has not happened yet' do
    bot = bot_trading('KLAC')
    split_row(at: 3.days.from_now)

    assert_empty bot.split_events
    assert_not bot.unresolved_split?, 'and it cannot stand the bot down either'
  end

  # Deleting the cache keys alone loses a race: a walk that read the ledger just before the split
  # was stored can finish just after and write its pre-split answer into the hole. Past the bump it
  # is computing an old key and can only ever write there.
  test 'expiring moves the keys somewhere a pre-split writer cannot reach' do
    bot = bot_trading('KLAC')
    before = bot.send(:metrics_cache_key)

    bot.expire_restated_metrics!

    assert_equal 1, bot.reload.restatement_generation
    assert_not_equal before, bot.send(:metrics_cache_key)
  end

  test 'a bot that has never been restated keeps one stable key' do
    bot = bot_trading('KLAC')

    assert_equal bot.send(:metrics_cache_key), bot.reload.send(:metrics_cache_key)
    assert_equal 0, bot.restatement_generation
  end
end
