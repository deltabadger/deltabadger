# frozen_string_literal: true

require 'test_helper'
require 'turbo/broadcastable/test_helper'

class Bots::DcaMultiAssetsRemovedAssetsTableTest < ActionDispatch::IntegrationTest
  include Turbo::Broadcastable::TestHelper

  setup do
    @user = create(:user, admin: true, setup_completed: true)
    sign_in @user
    @assets = %w[AAA BBB CCC].to_h do |symbol|
      asset = create(:asset, symbol:, name: "Coin #{symbol}", external_id: "coin-#{symbol.downcase}")
      [symbol, asset]
    end
    allocations = { @assets['AAA'] => 0.5, @assets['BBB'] => 0.3, @assets['CCC'] => 0.2 }
    @bot = create(:dca_multi_asset, user: @user, base_assets: allocations.keys, allocations:)
    @store = ActiveSupport::Cache::MemoryStore.new
    Rails.stubs(:cache).returns(@store)
  end

  test 'a removed asset moves under Removed from portfolio with a Sell button' do
    remove_member('CCC')
    warm_prices('AAA' => 50, 'BBB' => 30, 'CCC' => 20)

    get bot_path(id: @bot.id)

    assert_response :success
    assert_select '#exited_metrics_table .exited-header', text: /#{Regexp.escape(I18n.t(@bot.exited_title_key))}/
    assert_select '#exited_metrics_table tbody', /CCC/
    assert_select '#exited_metrics_table a[href=?][data-turbo-frame="modal"]',
                  new_bot_liquidation_path(bot_id: @bot.id, symbol: 'CCC', asset_id: asset_id_of('CCC')), count: 1
  end

  test 'Sell all sits over the removed-assets table too' do
    # The widget is shared with index bots; the band must not become index-only. A fourth member,
    # because a basket validates down to two and this removes two.
    @assets['DDD'] = create(:asset, symbol: 'DDD', name: 'Coin DDD', external_id: 'coin-ddd')
    allocations = @assets.values.index_with { 0.25 }
    @bot = create(:dca_multi_asset, user: @user, exchange: @bot.exchange, quote_asset: @bot.quote_asset,
                                    base_assets: allocations.keys, allocations: allocations)
    remove_member('BBB')
    remove_member('CCC')
    warm_prices('AAA' => 50, 'BBB' => 30, 'CCC' => 20, 'DDD' => 10)

    get bot_path(id: @bot.id)

    assert_select '.exited-header a[href=?][data-turbo-frame="modal"]',
                  new_bot_liquidation_path(bot_id: @bot.id, symbol: %w[BBB CCC], asset_id: asset_ids_of(%w[BBB CCC])), count: 1
  end

  test 'an in-portfolio asset carries a Sell button of its own' do
    # Any position can be closed on its own, a current member included.
    remove_member('CCC')
    warm_prices('AAA' => 50, 'BBB' => 30, 'CCC' => 20)

    get bot_path(id: @bot.id)

    assert_select '#assets_metrics_table', /AAA/
    assert_select '#assets_metrics_table a[href=?][data-turbo-frame="modal"]',
                  new_bot_liquidation_path(bot_id: @bot.id, symbol: 'AAA', asset_id: asset_id_of('AAA')), count: 1
  end

  test 'the removal broadcasts the metrics panel once' do
    stub_broadcast_metrics

    broadcasts = capture_turbo_stream_broadcasts(@bot.page_stream) do
      remove_member('CCC')
    end
    metrics_broadcasts = broadcasts.count { |broadcast| broadcast['target'] == 'metrics' }

    assert_equal 1, metrics_broadcasts
  end

  private

  def remove_member(symbol)
    stub_broadcast_metrics
    @bot.assign_attributes(@bot.parse_params(remove_asset_id: @assets.fetch(symbol).id))
    @bot.set_missed_quote_amount
    @bot.save!
  end

  def stub_broadcast_metrics
    stubbed = keyed_payload(
      realised_pnl: 0,
      prices_stale: false,
      total_quote_amount_invested: 0,
      total_amount_value_in_quote: 0,
      asset_values: {}
    )
    @bot.stubs(:metrics_with_current_prices).returns(stubbed)
  end

  def warm_prices(values)
    data = @bot.metrics.deep_dup
    data[:asset_values] = values.transform_values do |value|
      { amount: value.to_d / 100, quote_invested: value.to_d, current_value: value.to_d,
        current_price: 100, avg_price: 100, pnl_percentage: 0 }
    end
    data[:asset_breakdown] = values.transform_values do |value|
      { amount: value.to_d / 100, quote_invested: value.to_d }
    end
    data = keyed_payload(data)
    Rails.cache.write("bot_#{@bot.id}_metrics_v3", data)
    Rails.cache.write(@bot.send(:metrics_with_current_prices_cache_key), data)
  end
end
