# frozen_string_literal: true

require 'test_helper'

# A basket's members are named through the API the way its holdings are: by symbol when no other member
# shares it, else by a key that carries the asset id (POR#12). An allocation may also name an asset id. A
# bare symbol two members share names neither, and is refused.
class BotApi::Bots::BasketMemberKeysTest < ActiveSupport::TestCase
  setup do
    @user = create(:user)
    @fan = create(:asset, symbol: 'POR', name: 'Portugal Fan Token', external_id: 'por-fan')
    @portuma = create(:asset, symbol: 'POR', name: 'Portuma', external_id: 'portuma')
    @literal = create(:asset, symbol: "POR##{@fan.id}", name: 'Literal', external_id: 'literal')
    exchange = create(:binance_exchange)
    usd = @usd = create(:asset, :usd)
    { @fan => 'POR', @portuma => 'PORTUMA', @literal => 'PORX' }.each do |asset, spelling|
      create(:ticker, exchange:, base_asset: asset, quote_asset: usd, base_symbol: spelling)
    end
    @bot = create(:dca_multi_asset, :stopped, user: @user, exchange:, quote_asset: usd,
                                              base_assets: [@fan, @portuma, @literal])
  end

  test 'get lists a distinct key for every member, with the asset behind it' do
    data = BotApi::Bots::Get.call(user: @user, bot_id: @bot.id).data

    assert_equal 3, data[:allocations].keys.uniq.size
    assert_not_includes data[:allocations].keys, 'POR'
    assert_equal [@fan.id, @portuma.id, @literal.id].sort, data[:allocation_assets].map { |member| member[:asset_id] }.sort
    member = data[:allocation_assets].find { |entry| entry[:asset_id] == @portuma.id }
    assert_equal ['POR', 'Portuma', data[:allocations][member[:key]]], member.values_at(:symbol, :name, :weight)
  end

  test 'the keys get lists round-trip through update_settings' do
    keys = BotApi::Bots::Get.call(user: @user, bot_id: @bot.id).data[:allocation_assets].to_h { |m| [m[:asset_id], m[:key]] }

    result = update("#{keys[@fan.id]}:20,#{keys[@portuma.id]}:30,#{keys[@literal.id]}:50")

    assert result.success?, result.error_message
    assert_in_delta 0.3, @bot.reload.allocation_for(@portuma.id), 0.0001
  end

  test 'an asset id names its member' do
    result = update("#{@fan.id}:20,#{@portuma.id}:30,#{@literal.id}:50")

    assert result.success?, result.error_message
    assert_in_delta 0.5, @bot.reload.allocation_for(@literal.id), 0.0001
  end

  test "an identifier that is one member's key and another member's asset id is refused" do
    numeric = create(:asset, symbol: @fan.id.to_s, name: 'Numeric', external_id: 'numeric')
    create(:ticker, exchange: @bot.exchange, base_asset: numeric, quote_asset: @usd, base_symbol: 'NUMERIC')
    bot = create(:dca_multi_asset, :stopped, user: @user, exchange: @bot.exchange, quote_asset: @usd,
                                             base_assets: [@fan, numeric])

    result = BotApi::Bots::UpdateSettings.call(user: @user, bot_id: bot.id, allocations: "#{@fan.id}:50,NUMERIC:50")

    assert_equal 'ambiguous_basket_asset', result.error_code
  end

  test 'a bare symbol two members share is refused' do
    result = update("POR:50,#{@portuma.id}:30,#{@literal.id}:20")

    assert_equal 'ambiguous_basket_asset', result.error_code
  end

  test 'one asset named twice is refused' do
    btc = create(:asset, :bitcoin)
    eth = create(:asset, :ethereum)
    bot = create(:dca_multi_asset, :stopped, user: @user, exchange: @bot.exchange, quote_asset: @usd,
                                             base_assets: [btc, eth])

    result = BotApi::Bots::UpdateSettings.call(user: @user, bot_id: bot.id, allocations: "BTC:30,#{btc.id}:30,ETH:40")

    assert_equal 'invalid_allocations', result.error_code
  end

  private

  def update(allocations) = BotApi::Bots::UpdateSettings.call(user: @user, bot_id: @bot.id, allocations:)
end
