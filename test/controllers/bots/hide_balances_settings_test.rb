# frozen_string_literal: true

require 'test_helper'

# "Invest 100 USD / week into BTC" becomes "Invest USD / week into BTC": the number goes, the
# ticker and the interval stay. The field is still in the form as a hidden one — a sibling submit
# (changing the interval, dragging an allocation) posts the whole form, and a missing amount would
# blank the bot's contribution.
class Bots::HideBalancesSettingsTest < ActionDispatch::IntegrationTest
  setup do
    create(:user, admin: true, setup_completed: true) # satisfies the onboarding gate
    @user = create(:user, hide_balances: true)
    @bot = create(:dca_single_asset, user: @user)
    @bot.quote_amount = 250
    @bot.set_missed_quote_amount
    @bot.save!
    sign_in @user
  end

  test 'the contribution is a hidden field carrying its unchanged value' do
    get bot_path(id: @bot.id)

    assert_select 'input[type=number][name=?]', 'bots_dca_single_asset[quote_amount]', false
    assert_select 'input[type=hidden][name=?][value=?]',
                  'bots_dca_single_asset[quote_amount]', '250'
  end

  test 'the ticker and the interval stay' do
    get bot_path(id: @bot.id)

    assert_select '.conversational .ticker', text: @bot.quote_asset.symbol
    assert_select 'select[name=?]', 'bots_dca_single_asset[interval]'
  end

  test 'the contribution is a normal number field when balances are shown' do
    @user.update!(hide_balances: false)

    get bot_path(id: @bot.id)

    assert_select 'input[type=number][name=?]', 'bots_dca_single_asset[quote_amount]'
  end

  test 'the spend cap is hidden too, and so is what is left of it' do
    @bot.quote_amount_limited = true
    @bot.quote_amount_limit = 1000
    @bot.set_missed_quote_amount
    @bot.save!

    get bot_path(id: @bot.id)

    assert_select 'input[type=number][name=?]', 'bots_dca_single_asset[quote_amount_limit]', false
    assert_select 'input[type=hidden][name=?]', 'bots_dca_single_asset[quote_amount_limit]'
    assert_select '#settings-amount-limit-info', false
  end

  test 'the smart-interval slice is hidden too, and so is its sentence' do
    @bot.smart_intervaled = true
    @bot.smart_interval_quote_amount = 25
    @bot.set_missed_quote_amount
    @bot.save!

    get bot_path(id: @bot.id)

    assert_select 'input[type=number][name=?]',
                  'bots_dca_single_asset[smart_interval_quote_amount]', false
    assert_select 'input[type=hidden][name=?]',
                  'bots_dca_single_asset[smart_interval_quote_amount]'
    assert_select '#settings-smart-intervals-info', false
  end

  # A price is a fact about the market, not about the holder, so the trigger settings keep theirs.
  test 'a price limit keeps its number' do
    @bot.price_limited = true
    @bot.price_limit = 30_000
    @bot.set_missed_quote_amount
    @bot.save!

    get bot_path(id: @bot.id)

    assert_select 'input[type=number][name=?]', 'bots_dca_single_asset[price_limit]'
  end

  # The wizard renders these same partials, and there the amount field IS the question.
  test 'a bot being created still asks for its contribution in the open' do
    get new_bots_dca_single_assets_pick_exchange_path

    assert_response :success
    assert_select 'input[type=hidden][name=?]', 'bots_dca_single_asset[quote_amount]', false
  end
end
