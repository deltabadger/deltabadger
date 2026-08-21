# frozen_string_literal: true

require 'test_helper'
require 'turbo/broadcastable/test_helper'

# A tile's PnL broadcast replaces the whole `#pnl_...` node, so it has to carry BOTH formats.
# If it shipped only the percent, a reader who had switched the page to amounts would watch
# tiles silently flip back one by one as their jobs landed.
class Bot::BroadcastPnlFormatTest < ActiveSupport::TestCase
  include Turbo::Broadcastable::TestHelper

  setup do
    @bot = create(:dca_single_asset, :started)
    @bot.stubs(:metrics_with_current_prices).returns(
      { pnl: 0.25.to_d, total_quote_amount_invested: 100.to_d, total_amount_value_in_quote: 125.to_d }
    )
  end

  def tile_html
    streams = capture_turbo_stream_broadcasts(["user_#{@bot.user_id}", :bot_updates]) do
      @bot.broadcast_pnl_update
    end
    tile = streams.find { |s| s['target'] == @bot.dom_id(@bot, :pnl) }
    assert tile, 'expected a turbo-stream replacing the tile PnL'
    tile.to_html
  end

  test 'the tile broadcast carries both the percent and the USD amount' do
    html = tile_html

    assert_includes html, 'pnl-percent'
    assert_includes html, 'pnl-amount'
    assert_includes html, '25.00%'
    assert_includes html, '+$25.00'
  end

  # Unlike the index, this runs in a background job — it may fetch a rate rather than
  # leave a tile without its amount until the next warm pass.
  test 'the broadcast is allowed to fetch an uncached rate' do
    @bot.stubs(:quote_asset).returns(create(:asset, :eur))
    Utilities::Currency.expects(:exchange_rate).with(from: 'EUR', to: 'USD', cache_only: false)
                       .returns(Result::Success.new(1.1))

    assert_includes tile_html, '+$27.50'
  end

  # The broadcast renders outside a request, so it cannot read the account's currency off
  # `current_user` — it has to take it from the bot's owner.
  test 'the tile broadcast reads in the owner account currency' do
    @bot.user.update!(display_currency: 'PLN')
    Utilities::Currency.stubs(:exchange_rate).with(from: 'USD', to: 'USD', cache_only: false)
                       .returns(Result::Success.new(1.0))
    Utilities::Currency.stubs(:exchange_rate).with(from: 'USD', to: 'PLN')
                       .returns(Result::Success.new(4.0))

    assert_includes tile_html, '+100.00 zł'
  end
end
