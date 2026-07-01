require 'test_helper'

# M1 — manual ⇄ flip via BotsController#reverse (member POST).
class Bots::ReverseControllerTest < ActionDispatch::IntegrationTest
  TURBO_STREAM_ACCEPT = 'text/vnd.turbo-stream.html, text/html'.freeze

  setup do
    @user = create(:user, admin: true, setup_completed: true)
    sign_in @user
  end

  test 'POST reverse flips a non-executing bot to selling and re-renders the settings turbo-stream' do
    bot = create(:dca_single_asset, :started, user: @user)
    assert_predicate bot, :buying?

    post reverse_bot_path(id: bot.id), headers: { 'Accept' => TURBO_STREAM_ACCEPT }

    assert_response :success
    assert_match 'turbo-stream', response.body
    assert_predicate bot.reload, :selling?
    # the re-rendered settings show the sell sentence (sell_amount input) and a reverse control
    assert_match 'bots_dca_single_asset[sell_amount]', response.body
    assert_match reverse_bot_path(id: bot.id), response.body
    # issues #1/#2: the action + timing dropdowns are merged into one per-side `…_mode` select
    assert_match 'bots_dca_single_asset[sell_price_limit_mode]', response.body
    assert_match 'bots_dca_single_asset[sell_price_drop_limit_mode]', response.body
    assert_match 'bots_dca_single_asset[sell_moving_average_limit_mode]', response.body
    assert_match 'bots_dca_single_asset[sell_indicator_limit_mode]', response.body
    # the old separate per-side action select is gone
    assert_no_match 'bots_dca_single_asset[sell_price_limit_action]', response.body
    # issue #6: the reverse control is the SVG icon partial (not the ⇄ glyph)
    assert_match '<svg', response.body
    assert_no_match(/⇄/, response.body)
  end

  test 'POST reverse flips a selling bot back to buying' do
    bot = create(:dca_single_asset, :started, user: @user)
    bot.flip_direction!
    assert_predicate bot.reload, :selling?

    post reverse_bot_path(id: bot.id), headers: { 'Accept' => TURBO_STREAM_ACCEPT }

    assert_response :success
    assert_predicate bot.reload, :buying?
  end

  test 'POST reverse does NOT flip an executing bot (defers with a notice)' do
    bot = create(:dca_single_asset, :executing, user: @user)

    post reverse_bot_path(id: bot.id), headers: { 'Accept' => TURBO_STREAM_ACCEPT }

    assert_response :success
    assert_predicate bot.reload, :buying?, 'an executing bot must not flip mid-run'
  end

  test 'POST reverse only acts on the current user\'s bots' do
    other_user = create(:user)
    bot = create(:dca_single_asset, :started, user: other_user)

    post reverse_bot_path(id: bot.id), headers: { 'Accept' => TURBO_STREAM_ACCEPT }

    assert_redirected_to bots_path
    assert_predicate bot.reload, :buying?
  end

  # == confirmation before reversing while orders are open ==

  test 'the reverse control asks for confirmation when the bot has open orders' do
    bot = create(:dca_single_asset, :started, user: @user)
    create(:transaction, bot: bot, side: :buy, status: :submitted, external_status: :open,
                         external_id: 'o1', amount: 1, quote_amount: 100)

    get bot_path(id: bot.id)

    assert_response :success
    assert_match I18n.t('bot.reverse_confirm'), response.body
  end

  test 'the reverse control flips without confirmation when there are no open orders' do
    bot = create(:dca_single_asset, :started, user: @user)

    get bot_path(id: bot.id)

    assert_response :success
    assert_match reverse_bot_path(id: bot.id), response.body
    assert_no_match I18n.t('bot.reverse_confirm'), response.body
  end
end
