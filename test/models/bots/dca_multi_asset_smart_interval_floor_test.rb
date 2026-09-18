require 'test_helper'

# A one-asset basket keeps the single-asset bot's Smart Intervals floor, which never included the venue's
# minimum order: a split under it places orders the venue refuses, which are skipped and carried. The floor
# is validated on every save — stop, start, the tick's own status writes — so a floor the bot never had
# would wedge every single-asset bot converted with such a split. Wider baskets keep the venue floor.
class Bots::DcaMultiAssetSmartIntervalFloorTest < ActiveSupport::TestCase
  def setup
    @btc = create(:asset, :bitcoin)
    @eth = create(:asset, :ethereum)
  end

  test "a one-asset basket may split below the venue minimum, and takes the tick's status write and a stop" do
    bot = create(:dca_multi_asset, user: create(:user), base_assets: [@btc])
    split_below_venue_minimum(bot)

    assert_predicate bot, :valid?
    assert bot.update(status: :executing), bot.errors.full_messages.to_sentence
    assert bot.stop, bot.errors.full_messages.to_sentence
  end

  test 'a two-asset basket still refuses a split below the venue minimum' do
    bot = create(:dca_multi_asset, user: create(:user), base_assets: [@btc, @eth])
    split_below_venue_minimum(bot)

    assert_not bot.valid?
    assert bot.errors.key?(:smart_interval_quote_amount)
  end

  private

  # The factory's venue minimum is 10 quote; 5 per order sits above the frequency and precision floors of
  # 100 a day.
  def split_below_venue_minimum(bot)
    bot.set_missed_quote_amount
    bot.assign_attributes(smart_intervaled: true, smart_interval_quote_amount: 5)
  end
end
