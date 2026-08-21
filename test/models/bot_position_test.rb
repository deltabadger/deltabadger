# frozen_string_literal: true

require 'test_helper'

# The dashboard order is the user's, not the alphabet's. `position` carries it; this covers how a
# bot gets one and how the list reads it back.
class BotPositionTest < ActiveSupport::TestCase
  setup do
    @user = create(:user)
  end

  test 'a new bot lands after the ones the user already has' do
    first = create(:dca_single_asset, user: @user)
    second = create(:dca_single_asset, user: @user, exchange: first.exchange,
                                       base_asset: first.base_asset, quote_asset: first.quote_asset)

    assert_operator second.position, :>, first.position
    assert_equal [first, second], @user.bots.ordered.to_a
  end

  test 'a position is never left at the unset sentinel' do
    bot = create(:dca_single_asset, user: @user)

    assert_predicate bot.position, :positive?
  end

  # Positions are per-user, so another account filling up its dashboard must not push this user's
  # next bot to position 40 — or, worse, make the two lists share a numbering.
  test "another user's bots do not consume this user's positions" do
    other = create(:user)
    theirs = create(:dca_single_asset, user: other)
    3.times do
      create(:dca_single_asset, user: other, exchange: theirs.exchange,
                                base_asset: theirs.base_asset, quote_asset: theirs.quote_asset)
    end

    mine = create(:dca_single_asset, user: @user, exchange: theirs.exchange,
                                     base_asset: theirs.base_asset, quote_asset: theirs.quote_asset)

    assert_equal 1, mine.position
  end

  test 'ordered sorts by position, not by label' do
    first = create(:dca_single_asset, user: @user, label: 'Zulu')
    second = create(:dca_single_asset, user: @user, label: 'Alpha', exchange: first.exchange,
                                       base_asset: first.base_asset, quote_asset: first.quote_asset)

    assert_equal [first, second], @user.bots.ordered.to_a
  end

  # Two bots created in overlapping transactions both read the same `maximum(:position)`. The list
  # still has to come back in one stable order rather than flipping between page loads.
  test 'duplicate positions still read back in a stable order' do
    first = create(:dca_single_asset, user: @user)
    second = create(:dca_single_asset, user: @user, exchange: first.exchange,
                                       base_asset: first.base_asset, quote_asset: first.quote_asset)
    second.update_column(:position, first.position)

    assert_equal [first, second], @user.bots.ordered.to_a
    assert_equal [first, second], @user.bots.ordered.to_a
  end
end
