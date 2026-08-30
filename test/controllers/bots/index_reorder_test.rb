# frozen_string_literal: true

require 'test_helper'

# The dashboard grid is draggable. This covers the markup the Stimulus controller needs to exist
# at all — everything it hangs off is server-rendered.
class Bots::IndexReorderTest < ActionDispatch::IntegrationTest
  setup do
    create(:user, admin: true, setup_completed: true) # onboarding gate
    @user = create(:user)
    sign_in @user
  end

  test 'tiles render in position order, not label order' do
    first = create(:dca_single_asset, user: @user, label: 'Zulu')
    second = create(:dca_single_asset, user: @user, label: 'Alpha', exchange: first.exchange,
                                       base_asset: first.base_asset, quote_asset: first.quote_asset)

    get bots_path

    assert_response :success
    assert_equal [first.id, second.id], rendered_tile_ids
  end

  test 'a dragged order is what the next page load shows' do
    first = create(:dca_single_asset, user: @user)
    second = create(:dca_single_asset, user: @user, exchange: first.exchange,
                                       base_asset: first.base_asset, quote_asset: first.quote_asset)

    patch reorder_bots_path, params: { ids: [second.id, first.id] }
    get bots_path

    assert_equal [second.id, first.id], rendered_tile_ids
  end

  test 'the grid wires the reorder controller with its endpoint' do
    create_two_bots

    get bots_path

    assert_select '.itiles' do |grid|
      assert_includes grid.first['data-controller'].split, 'bots-reorder'
      assert_equal reorder_bots_path, grid.first['data-bots-reorder-url-value']
    end
  end

  # The grid already carries `broadcast--on-connect` whenever a tile's P&L is still cold.
  # Overwriting `data-controller` instead of appending to it would silently kill that refresh and
  # leave the tiles spinning until the next manual reload.
  test 'the grid keeps its broadcast controller and values alongside the reorder one' do
    create_two_bots

    get bots_path

    assert_select '.itiles' do |grid|
      controllers = grid.first['data-controller'].split

      assert_includes controllers, 'bots-reorder'
      assert_includes controllers, 'broadcast--on-connect'
      assert_equal 'pnl_update', grid.first['data-broadcast--on-connect-method-value']
      assert grid.first['data-broadcast--on-connect-method-args-value'].present?
    end
  end

  test 'each tile is draggable and carries its bot id' do
    bots = create_two_bots

    get bots_path

    bots.each do |bot|
      assert_select %(.bot-tile[draggable="true"][data-bot-id="#{bot.id}"]), 1
    end
  end

  # Without this the browser drags the *link* out of the page instead of starting our tile drag.
  test 'the tile link opts out of being dragged itself' do
    create_two_bots

    get bots_path

    assert_select '.bot-tile > a[draggable="false"]', 2
  end

  private

  def rendered_tile_ids
    css_select('.bot-tile[data-bot-id]').map { |tile| tile['data-bot-id'].to_i }
  end

  def create_two_bots
    first = create(:dca_single_asset, user: @user)
    second = create(:dca_single_asset, user: @user, exchange: first.exchange,
                                       base_asset: first.base_asset, quote_asset: first.quote_asset)
    [first, second]
  end
end
