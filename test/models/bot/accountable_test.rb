require 'test_helper'

class Bot::AccountableTest < ActiveSupport::TestCase
  include ActiveSupport::Testing::TimeHelpers

  # Overbuy regression: a submitted/unknown order (placed on the exchange but not
  # yet confirmed) carries quote_amount and MUST be counted as reserved spend, the
  # same as an open order. Otherwise the bot under-counts what it already committed
  # and overbuys on the next cycle while confirmation keeps failing.

  test 'pending_quote_amount reserves an open order by its ordered quote_amount' do
    bot = create(:dca_single_asset, :started) # effective_quote_amount 100, one interval
    assert_equal 100.0, bot.pending_quote_amount

    create(:transaction, bot: bot, status: :submitted, external_status: :open,
                         external_id: 'o1', quote_amount: 100, created_at: Time.current)
    bot.reload

    assert_equal 0, bot.pending_quote_amount
  end

  test 'pending_quote_amount reserves a submitted/unknown order by its ordered quote_amount' do
    bot = create(:dca_single_asset, :started)
    assert_equal 100.0, bot.pending_quote_amount

    create(:transaction, bot: bot, status: :submitted, external_status: :unknown,
                         external_id: 'u1', quote_amount: 100,
                         amount_exec: nil, quote_amount_exec: nil, created_at: Time.current)
    bot.reload

    assert_equal 0, bot.pending_quote_amount
  end

  # A stored bot missing a settings key gets it filled in on load (after_initialize). That fill is
  # not a settings change: counting it failed every save that doesn't call set_missed_quote_amount
  # (delete, start fresh) with a 500, and would move the carry window (settings_changed_at).
  %i[dca_single_asset dca_multi_asset dca_index].each do |factory|
    test "#{factory} loaded without a defaulted settings key can be deleted without counting the fill as a settings change" do
      bot = create(factory, status: :stopped)
      bot.update_column(:settings, bot.settings.except('smart_interval_quote_amount'))

      changed_at = bot.reload.settings_changed_at

      loaded = Bot.find(bot.id)
      assert_not loaded.settings_changed_since_load?
      assert loaded.destroy
      deleted = Bot.find(bot.id)
      assert_predicate deleted, :deleted?
      assert_equal changed_at, deleted.settings_changed_at
      assert_equal loaded.settings, deleted.settings_in_database, 'the fill reaches the row'
    end
  end

  test 'a real settings change on a filled bot still needs set_missed_quote_amount' do
    bot = create(:dca_single_asset, status: :stopped)
    bot.update_column(:settings, bot.settings.except('smart_interval_quote_amount'))

    loaded = Bot.find(bot.id)
    loaded.quote_amount = 250
    assert_predicate loaded, :settings_changed_since_load?
    assert_raises(RuntimeError) { loaded.save }
  end

  test 'a bot loaded without a defaulted settings key can be started fresh' do
    bot = create(:dca_single_asset, status: :stopped)
    bot.update_column(:settings, bot.settings.except('smart_interval_quote_amount'))

    assert Bot.find(bot.id).start(start_fresh: true)
  end

  # A capture on a bot whose load filled a default still restarts the carry window with it, as it
  # did before the fill stopped counting as a settings change — or the owed interval would be
  # counted twice, once in the window and once in the carry.
  test 'a capture on a filled bot keeps what is owed' do
    bot = create(:dca_single_asset, :started)
    bot.update_columns(settings: bot.settings.except('smart_interval_quote_amount'), started_at: 1.hour.ago,
                       settings_changed_at: nil, transient_data: { 'missed_quote_amount' => 0 })
    assert_equal 100.0, Bot.find(bot.id).pending_quote_amount

    loaded = Bot.find(bot.id)
    loaded.set_missed_quote_amount
    loaded.update!(label: 'renamed')

    assert_equal 100.0, Bot.find(bot.id).pending_quote_amount
  end

  test 'a filled key the row has since stored is a real setting again' do
    bot = create(:dca_single_asset, status: :stopped)
    bot.update_column(:settings, bot.settings.except('smart_interval_quote_amount'))

    loaded = Bot.find(bot.id)
    filled = loaded.smart_interval_quote_amount
    loaded.update_columns(settings: loaded.settings.merge('smart_interval_quote_amount' => filled * 2))
    loaded.smart_interval_quote_amount = filled

    assert_predicate loaded, :settings_changed_since_load?
  end

  test 'an in-place edit of a filled value is a real change' do
    bot = create(:dca_single_asset, status: :stopped)
    bot.update_column(:settings, bot.settings.except('price_limit_timing_condition'))

    loaded = Bot.find(bot.id)
    loaded.price_limit_timing_condition.replace('after')

    assert_predicate loaded, :settings_changed_since_load?
  end

  test 'a reloaded bot missing a defaulted settings key can be deleted' do
    bot = create(:dca_single_asset, status: :stopped)
    bot.update_column(:settings, bot.settings.except('smart_interval_quote_amount'))

    assert bot.reload.destroy
  end
end
