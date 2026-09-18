require 'test_helper'

# A held asset the bot cannot price — its pair delisted or disabled on the venue. The pair bot shows its
# last fill and flags the figures stale; a one-asset basket, which replaces it, must do the same rather
# than value the holding at nothing and read -100%.
class Bots::DcaMultiAssetDelistedMemberTest < ActiveSupport::TestCase
  def setup
    @base = create(:asset, :bitcoin)
    Exchanges::Binance.any_instance.stubs(:get_tickers_prices).returns(Result::Success.new({}))
  end

  test 'a one-asset basket whose pair was delisted is valued at its last fill and flagged stale' do
    bot = create(:dca_multi_asset, user: create(:user), base_assets: [@base])
    bought(bot)
    delist(bot)

    data = Bot.find(bot.id).metrics_with_current_prices(force: true)

    assert data[:prices_stale], 'the pair bot shows the stale notice here'
    assert_in_delta 100, data[:total_amount_value_in_quote].to_f, 1e-9, 'never zero: -100% was the bug'
  end

  test 'a one-asset basket values a delisted holding exactly as a pair bot on the same fills' do
    basket = create(:dca_multi_asset, user: create(:user), base_assets: [@base])
    pair = create(:dca_single_asset, user: basket.user, exchange: basket.exchange, base_asset: @base,
                                     quote_asset: basket.quote_asset)
    [basket, pair].each { |bot| bought(bot) }
    delist(basket)

    basket_value = Bot.find(basket.id).metrics_with_current_prices(force: true)[:total_amount_value_in_quote]
    pair_value = Bot.find(pair.id).metrics_with_current_prices(force: true)[:total_amount_value_in_quote]

    assert_in_delta pair_value.to_f, basket_value.to_f, 1e-9
  end

  test "a one-asset basket keeps its pair's precision after the pair was delisted" do
    bot = create(:dca_multi_asset, user: create(:user), base_assets: [@base])
    delist(bot)

    decimals = Bot.find(bot.id).decimals

    assert decimals[:quote], 'the page rounds with it'
    assert decimals[:base]
  end

  test 'a two-asset basket with an unpriced member is not flagged stale: its rebalancing is not stopped' do
    # prices_stale also gates the rebalance leg's targets, so a wider basket keeps today's reading.
    bot = create(:dca_multi_asset, user: create(:user), base_assets: [@base, create(:asset, :ethereum)])
    bought(bot)

    data = Bot.find(bot.id).metrics_with_current_prices(force: true)

    assert_not data[:prices_stale]
  end

  private

  def bought(bot, base: @base)
    create(:transaction, bot:, exchange: bot.exchange, status: :submitted, external_status: :closed, side: :buy,
                         transaction_type: 'REGULAR', external_id: "b-#{SecureRandom.hex(3)}", base: base.symbol,
                         quote: bot.quote_asset.symbol, price: 100, amount: 1, amount_exec: 1, quote_amount: 100,
                         quote_amount_exec: 100)
  end

  def delist(bot)
    bot.exchange.tickers.find_by!(base_asset: @base, quote_asset: bot.quote_asset).update!(available: false)
  end
end
