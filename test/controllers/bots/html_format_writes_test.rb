# frozen_string_literal: true

require 'test_helper'

# Update, stop and archive answer Turbo streams only. An HTML request is refused with 406 before
# anything is written, as the Rust port answers; Rails used to commit and then fail to render.
class Bots::HtmlFormatWritesTest < ActionDispatch::IntegrationTest
  setup do
    @user = create(:user, admin: true, setup_completed: true)
    sign_in @user
    @bot = create(:dca_multi_asset, user: @user)
  end

  test 'an HTML update writes nothing' do
    assert_refused { patch bot_path(id: @bot.id), params: { bots_dca_multi_asset: { label: 'Renamed' } } }
  end

  test 'an HTML stop writes nothing' do
    @bot.update_columns(status: Bot.statuses[:scheduled])
    assert_refused { patch bot_stop_path(bot_id: @bot.id) }
  end

  test 'an HTML archive writes nothing' do
    @bot.update_columns(status: Bot.statuses[:stopped])
    assert_refused { post bot_archive_path(bot_id: @bot.id) }
  end

  private

  def assert_refused(&block)
    before = @bot.reload.attributes
    assert_no_difference 'BotActivityLog.count', &block
    assert_response :not_acceptable
    assert_equal before, @bot.reload.attributes
  end
end
