# frozen_string_literal: true

require 'test_helper'

# Archiving is a stop that also hides the bot from the list: the bot keeps its transactions and
# stays reachable under the "Archived" filter, where its Start button reads "Reactivate".
class Bots::ArchiveTest < ActionDispatch::IntegrationTest
  setup do
    create(:user, admin: true, setup_completed: true) # onboarding gate
    @user = create(:user)
    @bot = create(:dca_single_asset, user: @user, status: :stopped)
    # A second bot keeps the index on the list instead of redirecting to the only bot.
    @other = create(:dca_single_asset, user: @user, status: :stopped, exchange: @bot.exchange,
                                       base_asset: @bot.base_asset, quote_asset: @bot.quote_asset)
    sign_in @user
    Bot::ActionJob.stubs(:perform_later)
    Bot::ActionJob.stubs(:set).returns(stub(perform_later: true))
    Bot::BroadcastAfterScheduledActionJob.stubs(:perform_later)
    Bots::DcaSingleAsset.any_instance.stubs(:check_missed_quote_amount_was_set).returns(true)
  end

  # == The menu entry and its confirmation ==

  test 'the bot menu offers Archive next to Delete' do
    get bot_path(id: @bot.id)

    assert_response :success
    assert_select "a[href='#{edit_bot_archive_path(bot_id: @bot.id)}']", text: I18n.t('button.archive')
  end

  test 'an already archived bot is not offered Archive again' do
    @bot.update!(status: :archived)

    get bot_path(id: @bot.id)

    assert_response :success
    assert_select "a[href='#{edit_bot_archive_path(bot_id: @bot.id)}']", count: 0
  end

  test 'the Archive entry opens a confirmation modal' do
    get edit_bot_archive_path(bot_id: @bot.id)

    assert_response :success
    assert_select '.modal .modal__title', text: @bot.label
    assert_select '.modal', text: /#{Regexp.escape(I18n.t('bot.messages.do_you_want_to_archive'))}/
    assert_select "form[action='#{bot_archive_path(bot_id: @bot.id)}'] button", text: I18n.t('button.archive')
  end

  # == Archiving ==

  test 'confirming archives the bot' do
    post bot_archive_path(bot_id: @bot.id), as: :turbo_stream

    assert_response :ok
    assert_predicate @bot.reload, :archived?
  end

  test 'archiving a working bot stops it first, so no tick is left scheduled' do
    @bot.update!(status: :scheduled, started_at: Time.current)
    Bots::DcaSingleAsset.any_instance.expects(:cancel_scheduled_action_jobs).at_least_once

    post bot_archive_path(bot_id: @bot.id), as: :turbo_stream

    assert_response :ok
    assert_predicate @bot.reload, :archived?
    assert @bot.stopped_at.present?
  end

  test 'a bot whose pair is gone can still be archived — that is the bot worth archiving' do
    Ticker.update_all(available: false)

    post bot_archive_path(bot_id: @bot.id), as: :turbo_stream

    assert_response :ok
    assert_predicate @bot.reload, :archived?
  end

  # The menu icon is the number of live bots. Archiving patches widgets into the page it was
  # clicked on rather than reloading it, so the count has to come back in the same response or the
  # icon goes on showing the number from before the click.
  test 'archiving sends the menu count back, one lower' do
    post bot_archive_path(bot_id: @bot.id), as: :turbo_stream

    assert_response :ok
    assert_select 'turbo-stream[target=bot-count] template text', text: '1'
  end

  # == What an archived bot looks like ==

  test 'an archived bot reads Archived and offers Reactivate instead of Start' do
    @bot.update!(status: :archived)

    get bot_path(id: @bot.id)

    assert_response :success
    assert_select '.bot-control__status__text', text: I18n.t('bot.status.archived')
    assert_select "form[action='#{bot_archive_path(bot_id: @bot.id)}'] button", text: I18n.t('button.reactivate')
    assert_select '.animicon--start', count: 0
    assert_select '.animicon--stop', count: 0
  end

  # == Reactivating ==

  test 'reactivating a bot that has run returns it to paused' do
    @bot.update!(status: :archived, started_at: 1.day.ago)

    delete bot_archive_path(bot_id: @bot.id), as: :turbo_stream

    assert_response :ok
    assert_predicate @bot.reload, :stopped?
    # The bot may have just left the list it was clicked in, so the page is refetched whole.
    assert_select 'turbo-stream[action="refresh"]', 1
  end

  test 'a bot whose pair is gone can be reactivated too, not just archived' do
    @bot.update!(status: :archived, started_at: nil)
    Ticker.update_all(available: false)

    delete bot_archive_path(bot_id: @bot.id), as: :turbo_stream

    assert_response :ok
    assert_predicate @bot.reload, :stopped?
  end

  test 'a stale Reactivate leaves a bot that is running again alone' do
    @bot.update!(status: :scheduled, started_at: Time.current)

    delete bot_archive_path(bot_id: @bot.id), as: :turbo_stream

    assert_response :ok
    assert_predicate @bot.reload, :scheduled?
  end

  # == The list ==

  test 'an archived bot is gone from All and shows up under Archived' do
    @bot.update!(status: :archived)

    get bots_path

    assert_response :success
    assert_select "a[href='#{bot_path(id: @bot.id)}']", count: 0
    assert_select "a[href='#{bot_path(id: @other.id)}']", count: 1

    get bots_path(filter: 'archived')

    assert_response :success
    assert_select "a[href='#{bot_path(id: @bot.id)}']", count: 1
    assert_select "a[href='#{bot_path(id: @other.id)}']", count: 0
  end

  test 'the Archived filter appears only once something is archived' do
    get bots_path

    assert_response :success
    assert_select "a.segmented__option[data-value='archived']", count: 0

    @bot.update!(status: :archived)
    get bots_path

    assert_response :success
    assert_select "a.segmented__option[data-value='archived']", count: 1
  end

  test 'a user whose every bot is archived still gets the list, not the first-bot screen' do
    @bot.update!(status: :archived)
    @other.update!(status: :archived)

    get bots_path

    assert_response :success
    assert_select "a.segmented__option[data-value='archived']", count: 1
    assert_select "a[href='#{new_bots_dca_single_assets_pick_buyable_asset_path}']", count: 0
  end

  test 'an archived bot is not offered in the bot switcher' do
    @other.update!(status: :archived)

    get bot_path(id: @bot.id)

    assert_response :success
    assert_select ".dropdown--bots a[href='#{bot_path(id: @other.id)}']", count: 0
  end

  # == Nothing resurrects an archived bot ==

  test 'a stale Start button cannot start an archived bot' do
    @bot.update!(status: :archived)

    patch bot_start_path(bot_id: @bot.id), params: { start_fresh: 'true' }, as: :turbo_stream

    assert_response :unprocessable_content
    assert_predicate @bot.reload, :archived?
  end

  test 'a stop that lands after archiving leaves the bot archived' do
    @bot.update!(status: :archived)

    patch bot_stop_path(bot_id: @bot.id), as: :turbo_stream

    assert_predicate @bot.reload, :archived?
  end

  test 'deleting an API key does not stop (and so unarchive) an archived bot' do
    @bot.update!(status: :archived)

    @bot.api_key.stop_dependent_bots!

    assert_predicate @bot.reload, :archived?
  end

  test 'an in-flight action job cannot flip an archived bot back to working' do
    @bot.update!(status: :archived)

    assert_not Bot::ActionJob.transition_working_bot!(@bot, 'executing')
    assert_predicate @bot.reload, :archived?
  end
end
