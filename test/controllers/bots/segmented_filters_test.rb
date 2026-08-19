# frozen_string_literal: true

require 'test_helper'

# The two filter rows are the same control as the chart's VALUE/RETURN switch, in its FLUID
# sizing: "All" and "Transactions" are nowhere near the same width, and the order filters do not
# even have a fixed number of options — they appear only for the categories a bot actually has.
class Bots::SegmentedFiltersTest < ActionDispatch::IntegrationTest
  setup do
    create(:user, admin: true, setup_completed: true) # onboarding gate
    @user = create(:user)
    sign_in @user
  end

  test 'the bots list filters are a fluid segmented control of links' do
    running = create(:dca_single_asset, user: @user, status: :waiting) # `working` is a scope
    # Share the assets and the exchange: the factory would otherwise build a second Bitcoin.
    create(:dca_single_asset, user: @user, status: :stopped, exchange: running.exchange,
                              base_asset: running.base_asset, quote_asset: running.quote_asset)

    get bots_path(filter: 'active')

    assert_response :success
    assert_select '.segmented.segmented--fluid', 1
    assert_select '.segmented__thumb', 1
    assert_select 'a.segmented__option', 3
    assert_select 'a.segmented__option.is-on[aria-current="page"]', 1 do |option|
      assert_equal 'active', option.first['data-value']
    end
  end

  test 'the order filters are a fluid segmented control the order-filter controller still drives' do
    bot = create(:dca_single_asset, user: @user)
    create(:transaction, bot: bot, status: :submitted, external_status: :closed)
    create(:transaction, bot: bot, status: :submitted, external_status: :cancelled, external_id: 'c1')

    get bot_path(id: bot.id)

    assert_response :success
    assert_select '.segmented.segmented--fluid', 1
    # all + transactions + cancelled; no waiting row exists, so no waiting option
    assert_select 'button.segmented__option', 3
    assert_select '[data-value="all"].is-on', 1
    assert_select 'button[data-value="cancelled"]', 1
  end

  test 'a bot with only one category of orders shows no filter at all' do
    bot = create(:dca_single_asset, user: @user)
    create(:transaction, bot: bot, status: :submitted, external_status: :closed)

    get bot_path(id: bot.id)

    assert_response :success
    assert_select 'button.segmented__option', 0
  end
end
