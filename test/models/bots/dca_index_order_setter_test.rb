require 'test_helper'

# What the buy leg does when it cannot price one of the index's own constituents.
#
# This became load-bearing when incumbents stopped being price-probed during the refresh: the probe
# used to double as a pre-flight filter for the buy leg, because a coin it could not price was
# dropped from the composition moments before set_orders ran. Now such a coin keeps its seat, so the
# tolerance has to live here instead.
class Bots::DcaIndexOrderSetterTest < ActiveSupport::TestCase
  include ExchangeMockHelpers

  def setup
    @bot = create(:dca_index, user: create(:user), with_api_key: true)
    @assets = %w[AAA BBB].to_h do |symbol|
      asset = create(:asset, symbol: symbol, name: "Coin #{symbol}", external_id: "coin-#{symbol.downcase}")
      ticker = create(:ticker, exchange: @bot.exchange, base_asset: asset, quote_asset: @bot.quote_asset)
      [symbol, { asset: asset, ticker: ticker }]
    end
    @bot.instance_variable_set(:@tickers, @assets.values.map { |a| a[:ticker] })
    index_membership('AAA' => 0.5, 'BBB' => 0.5)
  end

  test 'every constituent priced gets an order' do
    price_all(100)

    result = @bot.send(:get_orders_data, 100.to_d)

    assert_predicate result, :success?
    assert_equal %w[AAA BBB], result.data.map { |o| o[:ticker].base }.sort
    assert(result.data.all? { |o| o[:quote_amount].positive? })
  end

  test 'an at-balance composition spends the whole contribution' do
    price_all(100)

    result = @bot.send(:get_orders_data, 100.to_d)

    assert_predicate result, :success?
    assert_in_delta 100, result.data.sum { |o| o[:quote_amount] }, 0.0001
  end

  test 'four equal assets still spend the whole contribution' do
    add_members('CCC', 'DDD', weights: { 'AAA' => 0.25, 'BBB' => 0.25, 'CCC' => 0.25, 'DDD' => 0.25 })
    price_all(100)

    result = @bot.send(:get_orders_data, 100.to_d)

    assert_in_delta 100, result.data.sum { |o| o[:quote_amount] }, 0.0001
  end

  # R2: unequal weights, heavily drifted — exercises both caps.
  test 'a drifted composition never exceeds an asset offset or the contribution' do
    add_members('CCC', weights: { 'AAA' => 0.6, 'BBB' => 0.3, 'CCC' => 0.1 })
    hold('AAA' => 10, 'BBB' => 0, 'CCC' => 0) # AAA worth 1000 at price 100: way overweight
    price_all(100)

    result = @bot.send(:get_orders_data, 100.to_d)
    orders = result.data.index_by { |o| o[:ticker].base }

    assert_nil orders['AAA'] # overweight: no order
    # targets after adding 100: total 1100 → BBB 330, CCC 110; offsets 330 and 110; contribution 100
    assert_operator orders['BBB'][:quote_amount], :<=, 330
    assert_operator orders['CCC'][:quote_amount], :<=, 110
    assert_in_delta 100, orders.values.sum { |o| o[:quote_amount] }, 0.0001
    assert_in_delta 75, orders['BBB'][:quote_amount], 0.01 # 100 * 330/440
    assert_in_delta 25, orders['CCC'][:quote_amount], 0.01
  end

  # == a constituent that will not price ==
  #
  # Skipping it and buying the rest is not available: Step 2 builds total_current_value only from
  # the coins that priced, so dropping one understates the portfolio by its entire held value. On a
  # bot whose holdings dwarf a single contribution every survivor's target then lands below its
  # current value, every offset clamps to zero, and the tick buys NOTHING — silently. Refusing to
  # act on a price we do not have is the same answer Rebalancer#unpriced_holding? gives the
  # rebalance leg.

  test 'a constituent that will not price is retried rather than killing the tick' do
    price_all(100)
    unpriced('BBB', error: 'proxy 502')

    error = assert_raises(Client::TransientNetworkError) { @bot.send(:get_orders_data, 100.to_d) }

    assert_match(/BBB/, error.message)
    assert_match(/proxy 502/, error.message)
  end

  test 'a constituent that will not price writes no failed order' do
    # Nothing was placed — a `failed` Transaction row would say an order was attempted and rejected,
    # and it also suppresses Bot::ActionJob's execution_failed logging, so the tick died twice over
    # in silence. Same reasoning as placement_transient_error?.
    price_all(100)
    unpriced('BBB', error: 'proxy 502')

    assert_raises(Client::TransientNetworkError) { @bot.send(:get_orders_data, 100.to_d) }

    assert_empty @bot.transactions.failed
  end

  test 'an empty book is retryable too, not a bare RuntimeError' do
    # The concrete clients raise on a zero price ("Wrong ask price for X: 0.0") instead of returning
    # a Failure. Ticker#priced? has always rescued that; the order path let it escape as a
    # RuntimeError, which matches neither retry_on and so left the bot in :retrying with no job.
    price_all(100)
    raising('BBB', RuntimeError.new('Wrong ask price for BBB/USD: 0.0'))

    error = assert_raises(Client::TransientNetworkError) { @bot.send(:get_orders_data, 100.to_d) }

    assert_match(/Wrong ask price/, error.message)
  end

  test 'a transient error from the client keeps its own message and provenance' do
    # It is already the right class and already carries original_class, which is how the placement
    # guard tells a request that never left from one that may have landed. Re-wrapping would erase
    # that, so it is re-raised untouched.
    price_all(100)
    raising('BBB', Client::TransientNetworkError.new('connection reset', original_class: Errno::ECONNRESET))

    error = assert_raises(Client::TransientNetworkError) { @bot.send(:get_orders_data, 100.to_d) }

    assert_equal 'connection reset', error.message
    assert_equal Errno::ECONNRESET, error.original_class
  end

  test 'a rate limit stays a rate limit, so it keeps its own longer backoff' do
    price_all(100)
    raising('BBB', Client::RateLimitedError.new('EAPI:Rate limit exceeded'))

    assert_raises(Client::RateLimitedError) { @bot.send(:get_orders_data, 100.to_d) }
  end

  private

  def index_membership(weights)
    weights.each do |symbol, weight|
      BotIndexAsset.create!(bot: @bot, asset: @assets[symbol][:asset], ticker: @assets[symbol][:ticker],
                            target_allocation: weight, in_index: true, entered_at: Time.current)
    end
  end

  def add_members(*symbols, weights:)
    symbols.each do |symbol|
      asset = create(:asset, symbol: symbol, name: "Coin #{symbol}", external_id: "coin-#{symbol.downcase}")
      ticker = create(:ticker, exchange: @bot.exchange, base_asset: asset, quote_asset: @bot.quote_asset)
      @assets[symbol] = { asset: asset, ticker: ticker }
    end
    @bot.instance_variable_set(:@tickers, @assets.values.map { |a| a[:ticker] })
    @bot.bot_index_assets.destroy_all
    index_membership(weights)
  end

  def hold(holdings)
    asset_breakdown = holdings.transform_values { |amount| { amount: amount, quote_invested: 0 } }
    @bot.stubs(:metrics).with(force: true).returns(asset_breakdown: asset_breakdown)
  end

  # Stubbed on the exchange, not on the Ticker objects in @assets: current_allocations reloads its
  # own rows, so a stub on this instance never sees the call. Mocha matches the ticker argument by
  # ==, which for two AR objects with the same id is true.
  def price_all(price)
    %i[get_ask_price get_last_price].each do |method|
      Exchanges::Kraken.any_instance.stubs(method).returns(Result::Success.new(price.to_d))
    end
  end

  def unpriced(symbol, error:)
    Exchanges::Kraken.any_instance.stubs(:get_ask_price)
                     .with(ticker: @assets[symbol][:ticker], force: anything)
                     .returns(Result::Failure.new(error))
  end

  def raising(symbol, exception)
    Exchanges::Kraken.any_instance.stubs(:get_ask_price)
                     .with(ticker: @assets[symbol][:ticker], force: anything)
                     .raises(exception)
  end
end
