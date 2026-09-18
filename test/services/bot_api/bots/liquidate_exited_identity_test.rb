# frozen_string_literal: true

require 'test_helper'

# A holding is named by its key, by its asset id, or by a symbol no other holding shares. Whatever names it,
# the sale carries the asset the request meant, and a key that has come to mean another asset sells nothing.
class BotApi::Bots::LiquidateExitedIdentityTest < ActiveSupport::TestCase
  setup do
    @usd = create(:asset, :usd)
    @fan = create(:asset, symbol: 'POR', name: 'Portugal Fan Token', external_id: 'por-fan')
    @portuma = create(:asset, symbol: 'POR', name: 'Portuma', external_id: 'portuma')
    @mexc = create(:mexc_exchange)
    create(:ticker, exchange: @mexc, base_asset: @fan, quote_asset: @usd)
    create(:ticker, exchange: @mexc, base_asset: @portuma, quote_asset: @usd, base_symbol: 'PORTUMA')
    @user = create(:user)
    @bot = create(:dca_multi_asset, user: @user, exchange: @mexc, quote_asset: @usd, base_assets: [@fan, @portuma])
    [@fan, @portuma].each { |asset| bought(asset) }
    Exchanges::Mexc.any_instance.stubs(:market_open?).returns(true)
    Bots::DcaMultiAsset.any_instance.stubs(:ensure_exchange_authenticated)
  end

  test 'a symbol two holdings share is refused, naming their keys' do
    Bot::LiquidateExitedJob.expects(:perform_later).never

    result = call('POR')

    assert_equal 'holding_ambiguous', result.error_code
    assert_includes result.error_message, "POR##{@portuma.id}"
  end

  test 'a key or an asset id names one holding, and the job carries its asset' do
    [["POR##{@portuma.id}"], [@portuma.id.to_s]].each do |identifier|
      Bot::LiquidateExitedJob.expects(:perform_later)
                             .with(@bot, holdings: [["POR##{@portuma.id}", @portuma.id]], selling_token: anything)

      assert_predicate call(identifier), :success?
    end
  end

  test 'a key the page showed as another asset is not held' do
    Bot::LiquidateExitedJob.expects(:perform_later).never

    result = BotApi::Bots::LiquidateExited.call(user: @user, bot_id: @bot.id, symbol: "POR##{@fan.id}",
                                                asset_id: @portuma.id)

    assert_equal 'holding_not_held', result.error_code
  end

  test 'a queued sale whose key now names another asset sells nothing' do
    @bot.stubs(:liquidation_blocked_reason).returns(nil)
    @bot.stubs(:refresh_composition).returns(Result::Success.new)
    Exchanges::Mexc.any_instance.stubs(:get_tickers_prices)
                   .returns(Result::Success.new('PORUSD' => 1.to_d, 'PORTUMAUSD' => 10.to_d))
    @bot.expects(:create_order).never

    result = @bot.liquidate!(holdings: [["POR##{@fan.id}", @portuma.id]])

    assert_equal [:not_held], result.errors
  end

  private

  def call(symbol) = BotApi::Bots::LiquidateExited.call(user: @user, bot_id: @bot.id, symbol:)

  def bought(asset)
    create(:transaction, bot: @bot, exchange: @mexc, status: :submitted, external_status: :closed, side: :buy,
                         external_id: "b-#{asset.id}", base: 'POR', quote: 'USD', price: 10, amount: 10, amount_exec: 10,
                         quote_amount: 100, quote_amount_exec: 100, resolve_asset_ids: false,
                         base_asset_id: asset.id, quote_asset_id: @usd.id)
  end
end
