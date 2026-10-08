# frozen_string_literal: true

require 'test_helper'

# An interval, quote or exchange the app does not know is refused with 422 and writes nothing,
# as the Rust port answers; it used to raise after the parse and answer 500.
class Bots::InvalidSettingsParamsTest < ActionDispatch::IntegrationTest
  setup do
    @user = create(:user, admin: true, setup_completed: true)
    sign_in @user
    @bot = create(:dca_multi_asset, user: @user)
  end

  { 'interval' => 'bad', 'quote_asset_id' => '99999', 'exchange_id' => '99999' }.each do |field, value|
    test "an unknown #{field} is refused with 422 and nothing is written" do
      before = @bot.reload.attributes

      patch bot_path(id: @bot.id), params: { bots_dca_multi_asset: { field => value } }, as: :turbo_stream

      assert_response :unprocessable_entity
      assert_equal before, @bot.reload.attributes
    end
  end

  test 'an unknown interval on a smart-interval bot is refused with 422' do
    @bot.assign_attributes(smart_intervaled: true, smart_interval_quote_amount: 10)
    @bot.set_missed_quote_amount
    @bot.save!
    before = @bot.reload.attributes

    patch bot_path(id: @bot.id), params: { bots_dca_multi_asset: { interval: 'bad' } }, as: :turbo_stream

    assert_response :unprocessable_entity
    assert_equal before, @bot.reload.attributes
  end
end
