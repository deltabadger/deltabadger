require 'test_helper'

# The carry (missed_quote_amount) is what the bot owed up to the moment it was captured, and the
# carry window (settings_changed_at) restarts only when settings change. A capture saved without a
# settings change keeps the old window, so the owed intervals are counted twice — in the window and
# in the carry — and the next buy overspends by that much. An edit or lifecycle call that leaves
# settings alone must leave what the bot owes alone.
class Bots::CarryCaptureTest < ActionDispatch::IntegrationTest
  setup do
    @user = create(:user, admin: true, setup_completed: true)
    @bot = create(:dca_single_asset, user: @user, status: :stopped)
    # One day's contribution owed, nothing carried yet. Every default stored, as on any bot saved
    # since its concerns existed, so a load fills nothing in.
    @bot.update_columns(settings: Bot.find(@bot.id).settings, started_at: 1.hour.ago, settings_changed_at: nil,
                        transient_data: { 'missed_quote_amount' => 0 })
    assert_equal 100.0, owed
  end

  def owed = Bot.find(@bot.id).pending_quote_amount

  test 'renaming a bot in the settings form keeps what it owes' do
    sign_in @user
    patch bot_path(id: @bot.id), params: { bots_dca_single_asset: { label: 'renamed' } },
                                 headers: { 'Accept' => 'text/vnd.turbo-stream.html, text/html' }

    assert_equal 'renamed', @bot.reload.label
    assert_equal 100.0, owed
  end

  test 'a real settings edit in the form still captures the carry and restarts the window' do
    sign_in @user
    patch bot_path(id: @bot.id), params: { bots_dca_single_asset: { quote_amount: '250' } },
                                 headers: { 'Accept' => 'text/vnd.turbo-stream.html, text/html' }

    @bot.reload
    assert_equal 250, @bot.quote_amount
    assert_equal 100, @bot.missed_quote_amount, 'what was owed under the old amount is carried'
    assert_not_nil @bot.settings_changed_at
  end

  # The form changes no setting here; the save does, repointing the ticker ids at the new venue. The
  # capture is judged after that, so it lands with the window restart.
  test 'switching the exchange in the form captures the carry' do
    kraken = create(:kraken_exchange)
    create(:ticker, exchange: kraken, base_asset: @bot.base_asset, quote_asset: @bot.quote_asset)
    sign_in @user
    patch bot_path(id: @bot.id), params: { bots_dca_single_asset: { exchange_id: kraken.id } },
                                 headers: { 'Accept' => 'text/vnd.turbo-stream.html, text/html' }

    @bot.reload
    assert_equal kraken.id, @bot.exchange_id
    assert_equal 100, @bot.missed_quote_amount
    assert_not_nil @bot.settings_changed_at
    assert_equal 100.0, owed
  end

  test 'capturing twice before a save does not count the owed interval twice' do
    bot = Bot.find(@bot.id)
    bot.set_missed_quote_amount
    bot.set_missed_quote_amount
    bot.quote_amount = 250
    bot.save!

    assert_equal 100, Bot.find(@bot.id).missed_quote_amount
  end

  test 'a carry the caller assigns after capturing is kept' do
    bot = Bot.find(@bot.id)
    bot.update_columns(transient_data: { 'missed_quote_amount' => 300 })
    bot.set_missed_quote_amount
    bot.missed_quote_amount = nil
    bot.save!

    assert_equal 0, Bot.find(@bot.id).missed_quote_amount
  end

  # A limit pause changes no setting (condition_met_at is transient) but moves the window through the
  # decorated started_at, so its capture lands as it always has (Bot::PriceLimitable).
  # Also when a later settings edit, not started_at, draws the window: pausing still moves started_at.
  { 'started_at' => nil, 'a later settings edit' => 2.hours }.each do |drawn_by, settings_age|
    test "a limit pause keeps its capture (window drawn by #{drawn_by})" do
      @bot.update_columns(settings: @bot.settings.merge('price_limited' => true),
                          started_at: 25.hours.ago, settings_changed_at: settings_age&.ago,
                          transient_data: { 'missed_quote_amount' => 0,
                                            'price_limit_condition_met_at' => 26.hours.ago.iso8601 })
      bot = Bot.find(@bot.id)
      assert_predicate bot, :price_limited?
      bot.set_missed_quote_amount
      captured = bot.missed_quote_amount
      bot.update!(price_limit_condition_met_at: nil)

      assert_predicate captured, :positive?
      assert_equal captured, Bot.find(@bot.id).missed_quote_amount
    end
  end

  test 'renaming over the API keeps what the bot owes' do
    result = BotApi::Bots::UpdateSettings.call(user: @user, bot_id: @bot.id, label: 'renamed')

    assert_predicate result, :success?
    assert_equal 100.0, owed
  end

  test 'a real settings edit over the API carries what was owed under the old amount' do
    result = BotApi::Bots::UpdateSettings.call(user: @user, bot_id: @bot.id, quote_amount: '250')

    assert_predicate result, :success?
    @bot.reload
    assert_equal 250, @bot.quote_amount
    assert_equal 100, @bot.missed_quote_amount
    assert_not_nil @bot.settings_changed_at
  end

  test 'resuming over the API keeps what the bot owes' do
    assert_predicate BotApi::Bots::Start.call(user: @user, bot_id: @bot.id), :success?
    assert_equal 100.0, owed
  end

  test 'stopping over the API keeps what the bot owes' do
    @bot.update_columns(status: Bot.statuses[:scheduled])

    assert_predicate BotApi::Bots::Stop.call(user: @user, bot_id: @bot.id), :success?
    assert_equal 100.0, owed
  end

  test 'archiving and unarchiving over the API keeps what the bot owes' do
    assert_predicate BotApi::Bots::Archive.call(user: @user, bot_id: @bot.id), :success?
    assert_predicate BotApi::Bots::Unarchive.call(user: @user, bot_id: @bot.id), :success?
    assert_equal 100.0, owed
  end
end
