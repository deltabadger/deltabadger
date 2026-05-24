require 'test_helper'

class Bots::DcaIndexTest < ActiveSupport::TestCase
  include ExchangeMockHelpers

  setup do
    @exchange = create(:kraken_exchange)
    @quote = create(:asset, :eur)

    # Candidates in CoinGecko rank order. "coin-dead" sits between two live
    # coins so that filtering it must backfill from a lower-ranked candidate.
    @asset_a, @ticker_a = create_candidate('coin-a', 'AAA')
    @asset_dead, @ticker_dead = create_candidate('coin-dead', 'DEAD')
    @asset_b, @ticker_b = create_candidate('coin-b', 'BBB')
    @asset_c, @ticker_c = create_candidate('coin-c', 'CCC')

    stub_top_coins(%w[coin-a coin-dead coin-b coin-c])
  end

  test 'market-order index skips a pair with no ask price and backfills from the next candidate' do
    bot = create(:dca_index, exchange: @exchange, quote_asset: @quote)
    bot.num_coins = 2

    # All pairs priced on last; dead pair priced on last too — so if selection
    # wrongly probed last instead of ask, it would NOT be filtered.
    stub_all_priced(:get_last_price)
    stub_all_priced(:get_ask_price)
    stub_unpriced(:get_ask_price, @ticker_dead)

    result = bot.refresh_index_composition

    assert_predicate result, :success?
    assert_equal %w[coin-a coin-b], in_index_external_ids(bot)
    assert_not_includes in_index_external_ids(bot), 'coin-dead'
  end

  test 'limit-order index skips a pair with no last price and backfills from the next candidate' do
    bot = create(:dca_index, :limit_ordered, exchange: @exchange, quote_asset: @quote)
    bot.num_coins = 2

    stub_all_priced(:get_ask_price)
    stub_all_priced(:get_last_price)
    stub_unpriced(:get_last_price, @ticker_dead)

    result = bot.refresh_index_composition

    assert_predicate result, :success?
    assert_equal %w[coin-a coin-b], in_index_external_ids(bot)
    assert_not_includes in_index_external_ids(bot), 'coin-dead'
  end

  test 'a pair that raises on price (zero-price guard) is treated as unpriced and skipped' do
    bot = create(:dca_index, exchange: @exchange, quote_asset: @quote)
    bot.num_coins = 2

    stub_all_priced(:get_ask_price)
    Exchanges::Kraken.any_instance.stubs(:get_ask_price)
                     .with(ticker: @ticker_dead, force: anything)
                     .raises(RuntimeError.new('Wrong ask price for DEADEUR: 0.0'))

    result = bot.refresh_index_composition

    assert_predicate result, :success?
    assert_equal %w[coin-a coin-b], in_index_external_ids(bot)
  end

  test 'returns a failure when no candidate pair is tradeable' do
    bot = create(:dca_index, exchange: @exchange, quote_asset: @quote)
    bot.num_coins = 2

    stub_all_unpriced(:get_ask_price)

    result = bot.refresh_index_composition

    assert_predicate result, :failure?
    assert_empty bot.bot_index_assets.in_index
  end

  private

  def create_candidate(external_id, symbol)
    asset = create(:asset, external_id: external_id, symbol: symbol)
    ticker = create(:ticker, exchange: @exchange, base_asset: asset, quote_asset: @quote)
    [asset, ticker]
  end

  def stub_top_coins(external_ids)
    coins = external_ids.each_with_index.map do |id, i|
      { 'id' => id, 'market_cap' => (100 - i).to_f, 'current_price' => 1.0 }
    end
    MarketData.stubs(:get_top_coins).returns(Result::Success.new(coins))
  end

  def stub_all_priced(method)
    Exchanges::Kraken.any_instance.stubs(method).returns(Result::Success.new(BigDecimal('100')))
  end

  def stub_all_unpriced(method)
    Exchanges::Kraken.any_instance.stubs(method).returns(Result::Failure.new('zero'))
  end

  def stub_unpriced(method, ticker)
    Exchanges::Kraken.any_instance.stubs(method)
                     .with(ticker: ticker, force: anything)
                     .returns(Result::Failure.new('zero'))
  end

  def in_index_external_ids(bot)
    bot.bot_index_assets.in_index.includes(:asset).map { |bia| bia.asset.external_id }.sort
  end
end
