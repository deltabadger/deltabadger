# frozen_string_literal: true

require 'test_helper'

# The coins slider says what it was drawn at (num_coins_rendered) beside what it submits, so an
# untouched slider stores nothing — the settings form submits every field on any change, and the
# slider is drawn against what the venue lists right now.
class Bots::DcaIndexes::NumCoinsSliderTest < ActionDispatch::IntegrationTest
  setup do
    create(:user, admin: true, setup_completed: true) # onboarding gate
    @user = create(:user)
    @bot = create(:dca_index, user: @user, status: :stopped)
    Index.create!(external_id: 'nasdaq-100', source: Index::SOURCE_DELTABADGER, name: 'ND100',
                  top_coins: (1..101).map { |i| "s#{i}" })
    @bot.update_columns(settings: @bot.settings.merge('index_type' => 'category',
                                                      'index_category_id' => 'nasdaq-100',
                                                      'num_coins' => 20, 'hold_all' => false))
    sign_in @user
  end

  # The settings form is a Turbo Stream submit; the failure path re-renders through it.
  def submit(params)
    patch bot_path(id: @bot.id), params: { bots_dca_index: params },
                                 headers: { 'Accept' => 'text/vnd.turbo-stream.html' }
  end

  test 'a rejected save draws the slider at the count that is actually stored' do
    # Otherwise the re-rendered form reports a count that was never saved as the rendered one, and
    # the next submit — matching it — reads as an untouched slider and drops the change silently.
    submit(num_coins: '7', num_coins_rendered: '20', num_coins_ceiling: '101', quote_amount: '0')

    assert_response :unprocessable_entity
    assert_match(/num_coins_rendered\]"[^>]*value="20"/, response.body)
    assert_equal 20, @bot.reload.num_coins
  end

  test 'correcting the rejected field still saves the count the user picked' do
    submit(num_coins: '7', num_coins_rendered: '20', num_coins_ceiling: '101', quote_amount: '0')
    submit(num_coins: '7', num_coins_rendered: '20', num_coins_ceiling: '101', quote_amount: '250')

    assert_equal 7, @bot.reload.num_coins
  end
end
