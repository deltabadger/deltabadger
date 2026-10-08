# frozen_string_literal: true

require 'test_helper'

# A closed buy whose fill cost the venue never reported (NULL quote_amount_exec) leaves what is left of
# the spending cap unknown. The page leaves that figure out instead of answering 500.
class Bots::UnknownCapSpendTest < ActionDispatch::IntegrationTest
  setup do
    create(:user, admin: true, setup_completed: true)
    @user = create(:user)
    @bot = create(:dca_single_asset, user: @user)
    @bot.quote_amount_limited = true
    @bot.quote_amount_limit = 1000
    @bot.set_missed_quote_amount
    @bot.save!
    create(:transaction, bot: @bot, side: :buy, external_status: :closed, quote_amount_exec: nil)
    sign_in @user
  end

  test 'the bot page renders without what is left of the cap' do
    get bot_path(id: @bot.id)

    assert_response :success
    assert_select '#settings-amount-limit-info', text: ''
  end

  test 'the bot cannot start while the spend is unknown' do
    assert @bot.invalid?(:start)
    assert_includes @bot.errors.details[:settings], { error: :quote_amount_spent_unknown }
  end

  test 'the bot list renders' do
    create(:dca_single_asset, user: @user, exchange: @bot.exchange,
                              base_asset: @bot.base_asset, quote_asset: @bot.quote_asset) # one bot redirects to its page

    get bots_path

    assert_response :success
  end
end
