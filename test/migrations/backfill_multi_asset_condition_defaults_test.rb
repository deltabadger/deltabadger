require 'test_helper'
require Rails.root.join('db/migrate/20260828094446_backfill_multi_asset_condition_defaults')

# A basket bot saved before it carried the trading-condition concerns dirties its settings on load,
# which Bot::Accountable then refuses to save over. These pin that the backfill closes that without
# touching what the bot is owed.
class BackfillMultiAssetConditionDefaultsTest < ActiveSupport::TestCase
  def legacy_bot
    bot = create(:dca_multi_asset)
    bot.update_columns(
      settings: bot.settings.slice('quote_asset_id', 'quote_amount', 'interval', 'allocations')
    )
    bot
  end

  test 'a row written before the concerns existed can be saved again afterwards' do
    bot = legacy_bot

    assert_raises(RuntimeError) { Bot.find(bot.id).update!(label: 'renamed') }

    BackfillMultiAssetConditionDefaults.new.up

    assert_nothing_raised { Bot.find(bot.id).update!(label: 'renamed') }
  end

  test 'the backfill does not move the carry' do
    # missed_quote_amount is a transient_data accessor (accountable.rb:5), and
    # set_missed_quote_amount would RECOMPUTE it — which is why the backfill uses update_columns.
    bot = legacy_bot
    bot.update_columns(transient_data: bot.transient_data.merge('missed_quote_amount' => 42.0))

    BackfillMultiAssetConditionDefaults.new.up

    assert_equal 42.0, Bot.find(bot.id).missed_quote_amount.to_f
  end

  test 'a row that already carries the defaults is left untouched' do
    bot = create(:dca_multi_asset)
    before = Bot.find(bot.id).updated_at

    BackfillMultiAssetConditionDefaults.new.up

    assert_equal before.to_i, Bot.find(bot.id).updated_at.to_i
  end
end
