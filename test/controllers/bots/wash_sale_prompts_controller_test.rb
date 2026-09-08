require 'test_helper'

# The question is asked once per account, at the first thing that can sell — not at bot start, which
# would ask people who only ever buy. Three seams reach the user; these cover the modal itself and
# the full-render catch-all, and the sale that cannot proceed without the answer.
class Bots::WashSalePromptsControllerTest < ActionDispatch::IntegrationTest
  TURBO_STREAM_ACCEPT = 'text/vnd.turbo-stream.html, text/html'.freeze

  setup do
    create(:user, admin: true, setup_completed: true) # onboarding gate
    @user = create(:user)
    sign_in @user
    @buyer = create(:dca_single_asset, user: @user)
    @bot = create(:dca_single_asset, user: @user, exchange: @buyer.exchange,
                                     base_asset: Asset.find_by(symbol: 'BTC'),
                                     quote_asset: Asset.find_by(symbol: 'USD'))
    @bot.update_column(:settings, @bot.settings.merge('direction' => 'selling'))
  end

  test 'a sell-capable bot on an undecided account gets the question' do
    get new_bot_wash_sale_prompt_path(bot_id: @bot.id)

    assert_response :success
    assert_select 'form select[name=?]', 'wash_sale[jurisdiction]'
    assert_select "input[name='wash_sale[enabled]'][value='1']"
    assert_select "input[name='wash_sale[enabled]'][value='0']"
  end

  test 'an account that has already answered is not asked again' do
    @user.update!(wash_sale_enabled: false)

    get new_bot_wash_sale_prompt_path(bot_id: @bot.id)

    assert_response :success
    assert_select '.wash-sale-question', count: 0
  end

  test 'a bot that cannot sell is not a reason to ask' do
    get new_bot_wash_sale_prompt_path(bot_id: @buyer.id)

    assert_response :success
    assert_select '.wash-sale-question', count: 0
  end

  test 'accepting stores the window and decides' do
    post bot_wash_sale_prompt_path(bot_id: @bot.id),
         params: { wash_sale: { enabled: '1', jurisdiction: 'IE' } }

    @user.reload
    assert_predicate @user, :wash_sale_decided?
    assert_equal 28, @user.wash_sale_days
  end

  test 'declining decides without enforcing anything' do
    post bot_wash_sale_prompt_path(bot_id: @bot.id), params: { wash_sale: { enabled: '0' } }

    @user.reload
    assert_predicate @user, :wash_sale_decided?
    assert_equal 0, @user.wash_sale_days
  end

  test 'an answer with no choice in it changes nothing and keeps the question up' do
    post bot_wash_sale_prompt_path(bot_id: @bot.id), params: { wash_sale: { jurisdiction: 'IE' } }

    assert_response :unprocessable_entity
    assert_not_predicate @user.reload, :wash_sale_decided?
  end

  test 'a refused jurisdiction changes nothing' do
    post bot_wash_sale_prompt_path(bot_id: @bot.id),
         params: { wash_sale: { enabled: '1', jurisdiction: 'XX' } }

    assert_response :unprocessable_entity
    assert_not_predicate @user.reload, :wash_sale_decided?
  end

  test 'the bot page carries the question for a bot that can sell' do
    get bot_path(id: @bot.id)

    assert_response :success
    assert_select 'turbo-frame#modal[src=?]', new_bot_wash_sale_prompt_path(bot_id: @bot.id)
  end

  test 'the bot page of a buy-only bot does not' do
    get bot_path(id: @buyer.id)

    assert_response :success
    assert_select 'turbo-frame#modal:not([src])'
  end

  test 'the bot page stops carrying it once the account has answered' do
    @user.update!(wash_sale_enabled: true)

    get bot_path(id: @bot.id)

    assert_select 'turbo-frame#modal:not([src])'
  end

  test 'arming a sale over turbo_stream carries the question in the same answer' do
    post reverse_bot_path(id: @buyer.id), headers: { 'Accept' => TURBO_STREAM_ACCEPT }

    assert_match 'wash_sale', response.body,
                 'the action never re-renders the layout, so the modal has to be part of its answer'
  end
end
