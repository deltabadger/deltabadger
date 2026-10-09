# frozen_string_literal: true

require 'test_helper'

# The bot switcher names every other bot as its own page does, including a bot stored without a
# name: that one shows the name generated from what it holds.
class Bots::SwitcherLabelsTest < ActionDispatch::IntegrationTest
  setup do
    create(:user, admin: true, setup_completed: true) # satisfies the onboarding gate
    @bot = create(:dca_single_asset, :started, label: 'Mine')
    sign_in @bot.user
  end

  test 'another bot without a name is listed under its generated name' do
    other = create(:dca_single_asset, user: @bot.user, exchange: @bot.exchange, label: 'Placeholder',
                                      base_asset: Asset.find(@bot.base_asset_id), quote_asset: Asset.find(@bot.quote_asset_id))
    ['', nil].each do |blank|
      other.update_columns(label: blank)
      generated = Bot.find(other.id).label

      get bot_path(id: @bot.id)

      assert_response :ok
      assert_select "a.dropdown__item[href='#{bot_path(id: other.id)}']", text: generated
    end
  end
end
