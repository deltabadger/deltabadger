# frozen_string_literal: true

require 'test_helper'

# The dashboard sends the ids it is showing, in the order the user dragged them into. Everything
# hard about this endpoint is that "the ids it is showing" can be a *filtered subset*, so the bots
# that were off screen have to keep their place relative to the ones that were.
class Bots::ReordersControllerTest < ActionDispatch::IntegrationTest
  setup do
    create(:user, admin: true, setup_completed: true) # onboarding gate
    @user = create(:user)
    sign_in @user
  end

  test 'a full reorder persists and survives a reload' do
    bots = create_bots(3)

    patch reorder_bots_path, params: { ids: [bots[2].id, bots[0].id, bots[1].id] }

    assert_response :success
    assert_equal [bots[2], bots[0], bots[1]], @user.bots.ordered.to_a
  end

  test 'positions come back as a dense sequence' do
    bots = create_bots(3)

    patch reorder_bots_path, params: { ids: bots.reverse.map(&:id) }

    assert_equal [1, 2, 3], @user.bots.ordered.map(&:position)
  end

  # The grid is filtered, so a drop on ?filter=active sends only the active bots. Reordering those
  # must not shuffle the ones the filter hid.
  test 'a filtered reorder leaves the hidden bot in its slot relative to the visible ones' do
    first, hidden, third = create_bots(3)

    patch reorder_bots_path, params: { ids: [third.id, first.id] }

    # The two visible bots swap; `hidden` keeps the slot between them that it started in.
    assert_equal [third, hidden, first], @user.bots.ordered.to_a
  end

  # Two bots created in overlapping transactions can share a position. Redealing the slots those
  # bots already occupy could never express an order across the duplicate — the id tie-breaker
  # would win forever — so a save renumbers the whole list instead.
  test 'duplicate stored positions are normalized and the new order sticks' do
    first, second = create_bots(2)
    second.update_column(:position, first.position)

    patch reorder_bots_path, params: { ids: [second.id, first.id] }

    assert_equal [second, first], @user.bots.ordered.to_a
    assert_equal [1, 2], @user.bots.ordered.map(&:position)
  end

  test "another user's bot is ignored and never written" do
    mine = create_bots(2)
    stranger = create(:user)
    theirs = create(:dca_single_asset, user: stranger, exchange: mine[0].exchange,
                                       base_asset: mine[0].base_asset, quote_asset: mine[0].quote_asset)
    was = theirs.position

    patch reorder_bots_path, params: { ids: [theirs.id, mine[1].id, mine[0].id] }

    assert_response :success
    assert_equal [mine[1], mine[0]], @user.bots.ordered.to_a
    assert_equal was, theirs.reload.position
  end

  test 'duplicate and unknown ids do not corrupt the sequence' do
    first, second, third = create_bots(3)

    patch reorder_bots_path, params: { ids: [third.id, third.id, 999_999, 'nonsense', first.id] }

    assert_response :success
    assert_equal [third, second, first], @user.bots.ordered.to_a
  end

  test 'an empty payload changes nothing' do
    bots = create_bots(2)

    patch reorder_bots_path, params: { ids: [] }

    assert_response :success
    assert_equal bots, @user.bots.ordered.to_a
  end

  test 'a signed-out request writes nothing' do
    bots = create_bots(2)
    sign_out @user

    patch reorder_bots_path, params: { ids: bots.reverse.map(&:id) }

    assert_redirected_to new_user_session_path
    assert_equal bots, @user.bots.ordered.to_a
  end

  private

  # Share the exchange and assets: the factory would otherwise build a second Bitcoin per bot.
  def create_bots(count)
    first = create(:dca_single_asset, user: @user)
    rest = (count - 1).times.map do
      create(:dca_single_asset, user: @user, exchange: first.exchange,
                                base_asset: first.base_asset, quote_asset: first.quote_asset)
    end
    [first, *rest]
  end
end
