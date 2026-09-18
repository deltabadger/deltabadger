require 'test_helper'

# The one-time warning for an account that applies wash-sale protection, when a basket turns to
# selling: protection still stops Deltabadger buying an asset back after a loss sale, but it does not
# check what was bought BEFORE one — and a selling basket sells a bit at a time. Once per account, and
# only once the user acknowledges it.
class Bots::WashSaleSellingWarningsControllerTest < ActionDispatch::IntegrationTest
  include ActiveSupport::Testing::TimeHelpers

  TURBO_STREAM_ACCEPT = 'text/vnd.turbo-stream.html, text/html'.freeze

  setup do
    create(:user, admin: true, setup_completed: true) # onboarding gate
    @user = create(:user, wash_sale_enabled: true, wash_sale_jurisdiction: 'US')
    sign_in @user
    @basket = create(:dca_multi_asset, user: @user)
  end

  # == when it is due ==

  test 'reversing a basket into selling warns an account that applies the rule' do
    reverse(@basket)

    assert_predicate @basket.reload, :selling?
    assert_match warning_path(@basket), response.body
  end

  test 'an account that has not answered the question gets the question, not the warning' do
    @user.update!(wash_sale_enabled: nil)

    reverse(@basket)

    assert_match 'wash_sale[jurisdiction]', response.body
    assert_no_match warning_path(@basket), response.body
  end

  test 'an account that does not apply the rule is not warned' do
    @user.update!(wash_sale_enabled: false)

    reverse(@basket)

    assert_no_match warning_path(@basket), response.body
  end

  test 'reversing a basket back to buying does not warn' do
    sell!(@basket)

    reverse(@basket)

    assert_predicate @basket.reload, :buying?
    assert_no_match warning_path(@basket), response.body
  end

  test 'a pair bot is not warned' do
    pair = create(:dca_single_asset, user: @user, exchange: @basket.exchange,
                                     base_asset: @basket.base_assets.first, quote_asset: @basket.quote_asset)

    reverse(pair)

    assert_predicate pair.reload, :selling?
    assert_no_match 'wash_sale_selling_warning', response.body
  end

  test 'arming a start-selling condition on a basket warns: it is a reversal, just deferred' do
    patch bot_path(id: @basket.id), params: { bots_dca_multi_asset: arming }, as: :turbo_stream

    assert_response :success
    assert_predicate @basket.reload, :armed_to_start_selling?
    assert_match warning_path(@basket), response.body
  end

  test 'an arming the save rejected does not warn: an acknowledgement for it would silence the real one' do
    patch bot_path(id: @basket.id), params: { bots_dca_multi_asset: arming.merge(price_limit: '-5') },
                                    as: :turbo_stream

    assert_response :unprocessable_entity
    assert_not_predicate @basket.reload, :armed_to_start_selling?
    assert_no_match warning_path(@basket), response.body
  end

  # == where it reaches the user ==

  test 'the page of a basket that is already selling carries the warning' do
    sell!(@basket)

    get bot_path(id: @basket.id)
    assert_select 'turbo-frame#modal[src=?]', new_bot_wash_sale_prompt_path(bot_id: @basket.id)

    get new_bot_wash_sale_prompt_path(bot_id: @basket.id)
    assert_select 'form[action=?]', warning_path(@basket)
  end

  test 'switching to a selling basket in the bot frame brings the warning inside the frame' do
    # The header switcher replaces only this frame, so the layout's modal source is never looked at.
    sell!(@basket)

    get bot_path(id: @basket.id), headers: { 'Turbo-Frame' => 'bot' }

    assert_select 'turbo-frame#bot turbo-stream[action="replace"][target="modal"]', count: 1
  end

  test 'a full page load carries it through the modal frame alone, not twice' do
    sell!(@basket)

    get bot_path(id: @basket.id)

    assert_select 'turbo-frame#bot turbo-stream[target="modal"]', count: 0
  end

  # == acknowledging ==

  test 'acknowledging records the first time, and the warning stops' do
    sell!(@basket)
    first = Time.zone.parse('2026-09-18 10:00')

    travel_to(first) { post warning_path(@basket), headers: { 'Accept' => TURBO_STREAM_ACCEPT } }
    assert_response :no_content
    travel_to(first + 1.day) { post warning_path(@basket), headers: { 'Accept' => TURBO_STREAM_ACCEPT } }

    assert_equal first, @user.reload.wash_sale_selling_warned_at
    get bot_path(id: @basket.id)
    assert_select 'turbo-frame#modal:not([src])'
  end

  test "another account's bot is not found, and nothing is recorded" do
    other = create(:dca_multi_asset, user: create(:user), exchange: @basket.exchange,
                                     base_assets: @basket.base_assets, quote_asset: @basket.quote_asset)

    post warning_path(other), headers: { 'Accept' => TURBO_STREAM_ACCEPT }

    assert_response :not_found
    assert_nil @user.reload.wash_sale_selling_warned_at
  end

  private

  def warning_path(bot) = bot_wash_sale_selling_warning_path(bot_id: bot.id)

  def reverse(bot) = post(reverse_bot_path(id: bot.id), headers: { 'Accept' => TURBO_STREAM_ACCEPT })

  def sell!(bot)
    bot.set_missed_quote_amount
    bot.update!(direction: 'selling')
  end

  def arming
    ticker = @basket.exchange.tickers.find_by(base_asset: @basket.base_assets.first)
    { price_limited: '1', price_limit: '50000', price_limit_mode: 'flip', price_limit_in_ticker_id: ticker.id }
  end
end
