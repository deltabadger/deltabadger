require 'test_helper'

# "Your amount is too small for this venue" is said once, on a bot's first tick — the pair bot's rule —
# and for a one-asset basket in the pair bot's words, naming the asset and the venue's minimums. Whether
# a tick is the first is captured before placement: a row count cannot tell, because a two-member
# basket's ranked sell leg writes one skipped row per tick and parked or locked members underfill buys.
class Bots::DcaMultiAssetBelowMinimumsTest < ActiveSupport::TestCase
  def setup
    @base = create(:asset, :bitcoin)
  end

  test 'a first tick with one skipped order names the asset and the venue minimums' do
    bot = create(:dca_multi_asset, user: create(:user), base_assets: [@base])
    skipped(bot, @base)
    bot.expects(:broadcast_replace_to).with(
      ["user_#{bot.user_id}", :bot_updates],
      has_entries(target: 'modal', partial: 'bots/dca_single_assets/warning_below_minimums',
                  locals: has_entries(missed_symbol: @base.symbol))
    )

    bot.broadcast_below_minimums_warning(first_tick: true)
  end

  test 'a first tick where every member was skipped shows the basket warning' do
    ether = create(:asset, :ethereum)
    bot = create(:dca_multi_asset, user: create(:user), base_assets: [@base, ether])
    skipped(bot, @base)
    skipped(bot, ether)
    bot.expects(:broadcast_replace_to).with(
      ["user_#{bot.user_id}", :bot_updates],
      has_entries(target: 'modal', partial: 'bots/composition/warning_below_minimums',
                  locals: has_entries(skipped_count: 2))
    )

    bot.broadcast_below_minimums_warning(first_tick: true)
  end

  test 'a first tick that placed anything shows nothing' do
    bot = create(:dca_multi_asset, user: create(:user), base_assets: [@base])
    skipped(bot, @base)
    create(:transaction, bot:, exchange: bot.exchange, status: :submitted, external_status: :open, side: :buy,
                         external_id: 'placed', base: @base.symbol, quote: bot.quote_asset.symbol, price: 100, amount: 1)
    bot.expects(:broadcast_replace_to).never

    bot.broadcast_below_minimums_warning(first_tick: true)
  end

  test 'a later tick shows nothing, however it went' do
    bot = create(:dca_multi_asset, user: create(:user), base_assets: [@base])
    skipped(bot, @base)
    bot.expects(:broadcast_replace_to).never

    bot.broadcast_below_minimums_warning(first_tick: false)
  end

  test "a basket's execute_action tells the warning whether the tick was the bot's first" do
    bot = create(:dca_multi_asset, user: create(:user), base_assets: [@base])
    quiet_tick(bot)

    bot.expects(:broadcast_below_minimums_warning).with(first_tick: true)
    bot.execute_action
    skipped(bot, @base) # what that first tick wrote
    bot.expects(:broadcast_below_minimums_warning).with(first_tick: false)
    bot.execute_action
  end

  test "an index bot's execute_action does the same" do
    bot = create(:dca_index, user: create(:user))
    quiet_tick(bot)

    bot.expects(:broadcast_below_minimums_warning).with(first_tick: true)
    bot.execute_action
    create(:ticker, exchange: bot.exchange, base_asset: @base, quote_asset: bot.quote_asset)
    skipped(bot, @base)
    bot.expects(:broadcast_below_minimums_warning).with(first_tick: false)
    bot.execute_action
  end

  private

  def quiet_tick(bot)
    bot.stubs(:refresh_composition).returns(Result::Success.new)
    bot.stubs(:set_orders).returns(Result::Success.new)
  end

  def skipped(bot, asset)
    create(:transaction, bot:, exchange: bot.exchange, status: :skipped, external_id: nil, side: :buy,
                         base: asset.symbol, quote: bot.quote_asset.symbol, price: 100, amount: 0.001)
  end
end
