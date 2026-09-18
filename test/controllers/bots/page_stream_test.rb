require 'test_helper'

# A bot's own panels arrive on its own stream (Bot#page_stream). The bot switcher in the header swaps
# only the `bot` frame, so the subscription has to live inside it: outside, it would outlast the swap
# — the page would keep listening to the bot it left and never hear the one it shows.
class Bots::PageStreamTest < ActionDispatch::IntegrationTest
  setup do
    @user = create(:user, admin: true, setup_completed: true)
    sign_in @user
    @left = create(:dca_single_asset, user: @user)
    @shown = create(:dca_single_asset, user: @user, exchange: @left.exchange,
                                       base_asset: @left.base_asset, quote_asset: @left.quote_asset)
  end

  test 'the page listens to its bot inside the frame the switcher replaces' do
    get bot_path(id: @left.id)

    assert_select 'turbo-frame#bot turbo-cable-stream-source[signed-stream-name=?]', signed(@left), count: 1
    # Exactly once on the whole page: a copy outside the frame would survive switching bots.
    assert_select 'turbo-cable-stream-source[signed-stream-name=?]', signed(@left), count: 1
  end

  test 'switching bots brings the new bot\'s stream inside the frame, and not the old one' do
    # Turbo keeps only what is inside the requested frame, so that is the part that must carry it.
    get bot_path(id: @shown.id), headers: { 'Turbo-Frame' => 'bot' }

    assert_select 'turbo-frame#bot turbo-cable-stream-source[signed-stream-name=?]', signed(@shown), count: 1
    assert_select 'turbo-cable-stream-source[signed-stream-name=?]', signed(@left), count: 0
  end

  private

  def signed(bot) = Turbo::StreamsChannel.signed_stream_name(bot.page_stream)
end
