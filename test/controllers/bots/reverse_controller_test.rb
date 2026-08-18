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
    # the re-rendered settings show the sell sentence (sell_amount input) and the reverse control —
    # now LOCKED because the flipped bot is still running (rendered as the disabled span, no href)
    assert_match 'bots_dca_single_asset[sell_amount]', response.body
    assert_match 'reverse-toggle--disabled', response.body
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

  test 'POST reverse moves a base-selling bot on to quote-selling, then back to buying' do
    # The manual control rotates through three states, so a selling bot needs two POSTs to return
    # to buying (a TRIGGER flip is still a straight two-state flip — see Bot::RotationTest).
    bot = create(:dca_single_asset, :started, user: @user)
    bot.flip_direction!
    assert_predicate bot.reload, :sells_base_amount?

    post reverse_bot_path(id: bot.id), headers: { 'Accept' => TURBO_STREAM_ACCEPT }
    assert_response :success
    assert_predicate bot.reload, :sells_quote_amount?

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

  # == locked while running ==

  test 'the reverse control is locked (no clickable link) while the bot is running' do
    bot = create(:dca_single_asset, :started, user: @user) # working

    get bot_path(id: bot.id)

    assert_response :success
    assert_match 'reverse-toggle--disabled', response.body
    assert_no_match reverse_bot_path(id: bot.id), response.body # no reverse link/href while running
  end

  # == confirmation before reversing while orders are open (only for a non-running bot) ==

  test 'the reverse control asks for confirmation when a stopped bot has open orders' do
    bot = create(:dca_single_asset, :stopped, user: @user)
    create(:transaction, bot: bot, side: :buy, status: :submitted, external_status: :open,
                         external_id: 'o1', amount: 1, quote_amount: 100)

    get bot_path(id: bot.id)

    assert_response :success
    assert_match I18n.t('bot.reverse_confirm'), response.body
  end

  test 'the reverse control flips without confirmation when a stopped bot has no open orders' do
    bot = create(:dca_single_asset, :stopped, user: @user)

    get bot_path(id: bot.id)

    assert_response :success
    assert_match reverse_bot_path(id: bot.id), response.body
    assert_no_match I18n.t('bot.reverse_confirm'), response.body
  end

  # == three-state rotation (buy → sell N base → sell for N quote → buy) ==

  test 'three POSTs rotate the sentence through all three states' do
    bot = create(:dca_single_asset, :stopped, user: @user)

    post reverse_bot_path(id: bot.id), headers: { 'Accept' => TURBO_STREAM_ACCEPT }
    assert_response :success
    assert_match 'bots_dca_single_asset[sell_amount]', response.body
    assert_no_match 'bots_dca_single_asset[sell_quote_amount]', response.body

    post reverse_bot_path(id: bot.id), headers: { 'Accept' => TURBO_STREAM_ACCEPT }
    assert_response :success
    assert_predicate bot.reload, :sells_quote_amount?
    assert_match 'bots_dca_single_asset[sell_quote_amount]', response.body
    assert_no_match 'bots_dca_single_asset[sell_amount]"', response.body
    # Smart Intervals has no quote-sell form yet, so the rule is not offered in that mode
    assert_no_match 'bots_dca_single_asset[smart_intervaled]', response.body

    post reverse_bot_path(id: bot.id), headers: { 'Accept' => TURBO_STREAM_ACCEPT }
    assert_response :success
    assert_predicate bot.reload, :buying?
    assert_match 'bots_dca_single_asset[quote_amount]', response.body
    assert_match 'bots_dca_single_asset[smart_intervaled]', response.body
  end
end
