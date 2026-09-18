require 'test_helper'

# A composition bot's holdings are keyed by the asset each order traded, not by the symbol it recorded. A
# venue can spell an asset its own way (Kraken lists Bitcoin as XBT), two assets can share a symbol on one
# venue (MEXC's POR), and symbols get renamed: each of these used to split, merge or orphan a holding.
class AssetIdentityHoldingsTest < ActiveSupport::TestCase
  def setup
    @usd = create(:asset, :usd)
    @btc = create(:asset, :bitcoin)
    @eth = create(:asset, :ethereum)
    @user = create(:user)
  end

  # == Kraken lists Bitcoin as XBT; the rows say BTC ==

  test 'a Kraken basket values its BTC holding on the XBT ticker' do
    bot = kraken_basket
    fill(bot, 'BTC', amount: 1, price: 100)
    kraken_prices('XBTUSD' => 150, 'ETHUSD' => 10)

    values = bot.metrics_with_current_prices(force: true)[:asset_values]

    assert_in_delta 150, values.dig('BTC', :current_value).to_f, 1e-9
  end

  test 'a one-asset Kraken basket holding BTC is priced, not stale' do
    bot = kraken_basket(@btc)
    fill(bot, 'BTC', amount: 1, price: 100)
    kraken_prices('XBTUSD' => 150)

    data = bot.metrics_with_current_prices(force: true)

    assert_not data[:prices_stale]
    assert_in_delta 150, data[:total_amount_value_in_quote].to_f, 1e-9
  end

  test 'a Kraken BTC holding is sellable on the XBT ticker' do
    bot = kraken_basket
    fill(bot, 'BTC', amount: 1, price: 100)
    kraken_prices('XBTUSD' => 150, 'ETHUSD' => 10)

    holding = bot.sellable_holdings(bot.metrics_with_current_prices(force: true)).find { |h| h[:symbol] == 'BTC' }

    assert_equal [@xbt, @btc.id], [holding[:ticker], holding[:asset_id]]
  end

  test "a loss sale of Kraken BTC is judged on the holding's lots" do
    bot = kraken_basket
    wash_sale_on(bot)
    fill(bot, 'BTC', amount: 1, price: 100)

    assert bot.sell_at_loss?(ticker: @xbt, amount: 1, quote_amount: 50)
  end

  test 'a lot recorded without its asset, under a name of the asset being sold, reads as a possible loss' do
    bot = kraken_basket
    wash_sale_on(bot)
    fill(bot, 'XBT', amount: 1, price: 100, asset: nil) # before orders stored their asset
    fill(bot, 'BTC', amount: 1, price: 10)
    sale = fill(bot, 'BTC', amount: 1, price: 50, side: :sell)

    assert bot.sell_at_loss?(ticker: @xbt, amount: 1, quote_amount: 50), 'FIFO may consume the 100 lot'
    assert_nil bot.metrics(force: true)[:loss_lot_by_transaction][sale.id], 'unknown, which locks'
    bot.reconcile_wash_sale_from_fill!(sale)
    assert_includes bot.user.locked_asset_ids, @btc.id
  end

  test 'a history recorded entirely without its asset still reads as a possible loss' do
    bot = kraken_basket(@btc)
    wash_sale_on(bot)
    fill(bot, 'XBT', amount: 1, price: 100, asset: nil)

    assert bot.sell_at_loss?(ticker: @xbt, amount: 1, quote_amount: 50)
  end

  test 'a filled Kraken BTC loss sale locks BTC' do
    bot = kraken_basket
    wash_sale_on(bot)
    fill(bot, 'BTC', amount: 1, price: 100)
    sale = fill(bot, 'BTC', amount: 1, price: 50, side: :sell)

    bot.reconcile_wash_sale_from_fill!(sale)

    assert_includes bot.user.locked_asset_ids, @btc.id
  end

  test 'a resting buy of the asset under either name blocks a sale, on any bot and venue of the account' do
    bot = kraken_basket
    other = create(:dca_multi_asset, user: @user, quote_asset: @usd, base_assets: [@btc, @eth])
    resting = fill(other, 'BTC', amount: 1, price: 100, status: :open)
    Bot::FetchAndUpdateOpenOrdersJob.stubs(:perform_now)

    assert bot.send(:waiting_buy_blocks_sell?, @xbt), 'recorded as BTC, with its id'

    resting.update_columns(base: 'XBT', base_asset_id: nil)
    assert bot.send(:waiting_buy_blocks_sell?, @xbt), 'recorded under the venue spelling, no id'

    resting.update_columns(base: 'BTC', base_asset_id: create(:asset, symbol: 'BTC', name: 'Other', external_id: 'o').id)
    assert bot.send(:waiting_buy_blocks_sell?, @xbt), 'a matching name with another id still blocks: matching more only blocks more'
  end

  test "a resting buy without its asset, under another venue's name for it, blocks a sale" do
    kraken_basket # the XBT ticker
    binance_bot = create(:dca_multi_asset, user: @user, quote_asset: @usd, base_assets: [@btc, @eth])
    binance_btc = Ticker.find_by!(exchange: binance_bot.exchange, base_asset: @btc, quote_asset: @usd)
    kraken_bot = create(:dca_multi_asset, user: @user, exchange: @kraken, quote_asset: @usd, base_assets: [@btc, @eth])
    fill(kraken_bot, 'XBT', amount: 1, price: 100, status: :open, asset: nil)
    Bot::FetchAndUpdateOpenOrdersJob.stubs(:perform_now)

    assert binance_bot.send(:waiting_buy_blocks_sell?, binance_btc)
  end

  # == Two assets called POR on MEXC ==

  test "an index bot trades its POR member on that member's own ticker, never the other POR's" do
    fan, portuma, mexc = por_assets
    index = create(:dca_index, user: @user, exchange: mexc, quote_asset: @usd)
    member(index, portuma, @portuma_ticker, 0.5)
    member(index, @btc, create(:ticker, exchange: mexc, base_asset: @btc, quote_asset: @usd), 0.5)
    fill(index, 'POR', amount: 1, price: 10, asset: portuma)
    mexc_prices('PORTUMAUSD' => 10, 'PORUSD' => 1, 'BTCUSD' => 100)

    entry = index.send(:composition_targets, locked: []).find { |e| e[:ticker].base_asset_id == portuma.id }
    holding = index.sellable_holdings(index.metrics_with_current_prices(force: true)).find { |h| h[:asset_id] == portuma.id }

    assert_equal ['PORTUMA', 10], [entry[:ticker].base, entry[:value].to_i]
    assert_equal @portuma_ticker, holding[:ticker]
    assert_not_equal fan.id, holding[:asset_id]
  end

  test 'a basket holding both POR assets keeps two holdings, each priced on its own ticker' do
    fan, portuma, mexc = por_assets
    bot = create(:dca_multi_asset, user: @user, exchange: mexc, quote_asset: @usd, base_assets: [fan, portuma])
    fill(bot, 'POR', amount: 1, price: 1, asset: fan)
    fill(bot, 'POR', amount: 1, price: 10, asset: portuma)
    mexc_prices('PORUSD' => 2, 'PORTUMAUSD' => 20)

    values = bot.metrics_with_current_prices(force: true)[:asset_values]

    assert_equal({ "POR##{fan.id}" => 2, "POR##{portuma.id}" => 20 }, values.transform_values { |v| v[:current_value].to_i })
  end

  test 'a contribution sizes each POR member on its own holding' do
    fan, portuma, mexc = por_assets
    bot = create(:dca_multi_asset, user: @user, exchange: mexc, quote_asset: @usd, base_assets: [fan, portuma])
    fill(bot, 'POR', amount: 1, price: 10, asset: fan)
    Ticker.any_instance.stubs(:get_ask_price).returns(Result::Success.new(10.to_d))

    orders = bot.send(:get_orders_data, 10).data

    assert_equal({ portuma.id => 10 }, orders.to_h { |o| [o[:ticker].base_asset_id, o[:quote_amount].to_i] },
                 'the fan token is at target; the whole contribution buys Portuma')
  end

  # == Renames ==

  test 'rows recorded under an old and a new symbol are one holding, sold in order' do
    bot = kraken_basket
    wash_sale_on(bot)
    fill(bot, 'OLDBTC', amount: 1, price: 100, asset: @btc)
    fill(bot, 'BTC', amount: 1, price: 10, asset: @btc)
    sale = fill(bot, 'BTC', amount: 1, price: 50, side: :sell)

    data = bot.metrics(force: true)

    assert_equal ['BTC'], data[:asset_breakdown].keys
    assert_in_delta 1, data[:asset_breakdown]['BTC'][:amount].to_f, 1e-9
    assert data[:loss_lot_by_transaction][sale.id], 'the sale consumed the 100 lot, bought first'
  end

  test 'a CSV import under the venue spelling and a later bot sale are one holding' do
    bot = kraken_basket
    csv = "Timestamp,Order ID,Type,Side,Amount,Value,Price,Base Asset,Quote Asset,Status\n" \
          "2025-01-15 12:00:00,i-1,Market,Buy,2,200,100,XBT,USD,closed\n"
    bot.import_orders_csv(StringIO.new(csv))
    fill(bot, 'BTC', amount: 1, price: 100, side: :sell)

    breakdown = bot.metrics(force: true)[:asset_breakdown]

    assert_equal ['BTC'], breakdown.keys
    assert_in_delta 1, breakdown['BTC'][:amount].to_f, 1e-9
  end

  # == The chart ==

  test "a basket's buy marks and their logos are keyed like its holdings" do
    fan, portuma, mexc = por_assets
    bot = create(:dca_multi_asset, user: @user, exchange: mexc, quote_asset: @usd, base_assets: [fan, portuma])
    fill(bot, 'POR', amount: 1, price: 1, asset: fan)
    fill(bot, 'POR', amount: 1, price: 10, asset: portuma)
    keys = ["POR##{fan.id}", "POR##{portuma.id}"]

    assert_equal(keys, bot.chart_buy_marks.map { |mark| mark[1] })
    assert_equal({ keys[0] => fan, keys[1] => portuma }, bot.chart_logo_assets(keys))
  end

  test "a Kraken pair bot's marks sit under its ticker, beside its prices" do
    kraken_basket # the XBT ticker
    pair = create(:dca_single_asset, user: @user, exchange: @kraken, base_asset: @btc, quote_asset: @usd)
    fill(pair, 'BTC', amount: 1, price: 100)

    assert_equal(['XBT'], pair.chart_buy_marks.map { |mark| mark[1] })
    assert_equal({ 'XBT' => @btc }, pair.chart_logo_assets(['XBT']))
  end

  # == Order history ==

  test "a Kraken BTC order rounds on the XBT ticker's precision" do
    bot = kraken_basket
    @xbt.update!(base_decimals: 5)
    order = fill(bot, 'BTC', amount: 1, price: 100)

    decimals = BotsController.new.send(:composition_decimals, bot)

    assert_equal 5, Object.new.extend(BotHelper).send(:order_decimals, decimals, order, :base)
  end

  # == Rows recorded before orders stored their asset ==

  test 'an unresolved history reads by its string and is never offered for sale' do
    bot = create(:dca_multi_asset, user: @user, quote_asset: @usd, base_assets: [@btc, @eth])
    fill(bot, 'BTC', amount: 1, price: 100, asset: nil)
    Exchanges::Binance.any_instance.stubs(:get_tickers_prices).returns(Result::Success.new('BTCUSD' => 150))

    data = bot.metrics_with_current_prices(force: true)

    assert_in_delta 150, data[:asset_values].dig('BTC', :current_value).to_f, 1e-9, 'valued as before'
    assert_empty bot.sellable_holdings(data)
    assert_empty bot.held_symbols
  end

  test 'an unresolved BTC beside a resolved one is its own holding until a poll fills its id' do
    bot = kraken_basket
    fill(bot, 'BTC', amount: 1, price: 100)
    legacy = fill(bot, 'BTC', amount: 2, price: 100, asset: nil)
    kraken_prices('XBTUSD' => 150, 'ETHUSD' => 10)

    data = bot.metrics_with_current_prices(force: true)
    assert_equal ["BTC##{@btc.id}", 'BTC#?'].sort, data[:asset_breakdown].keys.sort
    assert_not data[:asset_values].key?('BTC#?'), 'no Kraken ticker is spelled BTC'
    assert_equal ["BTC##{@btc.id}"], bot.held_symbols

    legacy.update!(base_asset_id: @btc.id)
    assert_equal({ 'BTC' => 3 }, bot.metrics(force: true)[:asset_breakdown].transform_values { |h| h[:amount].to_i })
  end

  test 'a wider basket with a member the bulk read missed stands its rebalance down' do
    bot = kraken_basket
    fill(bot, 'BTC', amount: 1, price: 100)
    fill(bot, 'ETH', amount: 1, price: 10)
    kraken_prices('ETHUSD' => 10)

    assert_nil bot.send(:composition_targets, locked: [])
  end

  # == Resting orders with no asset yet ==

  test 'a resting sell with no asset is reserved against the holding its name matches' do
    bot = kraken_basket
    fill(bot, 'BTC', amount: 1, price: 100, side: :sell, status: :open, asset: nil)

    assert_equal({ @btc.id => 1 }, bot.send(:reserved_waiting_amounts, :sell).transform_values(&:to_i))
  end

  test 'a resting order whose name matches no asset of the bot stands the leg down until a poll fills its id' do
    bot = kraken_basket
    resting = fill(bot, 'RENAMED', amount: 1, price: 100, status: :open, asset: nil)

    assert_raises(Client::TransientNetworkError) { bot.send(:reserved_waiting_amounts, :buy) }

    resting.update_columns(base_asset_id: @btc.id)
    assert_equal({ @btc.id => 1 }, bot.send(:reserved_waiting_amounts, :buy).transform_values(&:to_i))
  end

  private

  def kraken_basket(*members)
    @kraken ||= create(:kraken_exchange)
    @xbt ||= create(:ticker, exchange: @kraken, base_asset: @btc, quote_asset: @usd, base_symbol: 'XBT')
    create(:dca_multi_asset, user: @user, exchange: @kraken, quote_asset: @usd,
                             base_assets: members.presence || [@btc, @eth])
  end

  def por_assets
    fan = create(:asset, symbol: 'POR', name: 'Portugal Fan Token', external_id: 'por-fan')
    portuma = create(:asset, symbol: 'POR', name: 'Portuma', external_id: 'portuma')
    mexc = create(:mexc_exchange)
    create(:ticker, exchange: mexc, base_asset: fan, quote_asset: @usd)
    @portuma_ticker = create(:ticker, exchange: mexc, base_asset: portuma, quote_asset: @usd, base_symbol: 'PORTUMA')
    [fan, portuma, mexc]
  end

  def member(bot, asset, ticker, weight)
    BotIndexAsset.create!(bot:, asset:, ticker:, target_allocation: weight, in_index: true, entered_at: Time.current)
  end

  # A row as the bot writes it: the asset from the ticker, the symbol as shown. asset: nil is a row recorded
  # before orders stored their asset.
  def fill(bot, base, amount:, price:, side: :buy, status: :closed, asset: :resolve)
    ids = asset == :resolve ? {} : { resolve_asset_ids: false, base_asset_id: asset&.id, quote_asset_id: @usd.id }
    create(:transaction, bot:, exchange: bot.exchange, status: :submitted, external_status: status, side:,
                         external_id: "t-#{SecureRandom.hex(4)}", base:, quote: 'USD', price:, amount:,
                         amount_exec: status == :closed ? amount : nil, quote_amount: amount * price,
                         quote_amount_exec: status == :closed ? amount * price : nil, **ids)
  end

  def wash_sale_on(bot) = bot.user.update!(wash_sale_enabled: true, wash_sale_jurisdiction: 'US')

  def kraken_prices(prices)
    Exchanges::Kraken.any_instance.stubs(:get_tickers_prices).returns(Result::Success.new(prices.transform_values(&:to_d)))
  end

  def mexc_prices(prices)
    Exchanges::Mexc.any_instance.stubs(:get_tickers_prices).returns(Result::Success.new(prices.transform_values(&:to_d)))
  end
end
