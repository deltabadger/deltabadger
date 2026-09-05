# frozen_string_literal: true

require 'test_helper'

class BotApi::Bots::AnswerRedeployTest < ActiveSupport::TestCase
  setup do
    @user = create(:user)
    @bot = create(:dca_index, user: @user, status: :scheduled, started_at: Time.current, with_api_key: true)
    Bots::DcaIndex.any_instance.stubs(:redeploy_offer).returns(25.to_d)
    Bots::DcaIndex.any_instance.stubs(:ensure_exchange_authenticated)
    Bots::DcaIndex.any_instance.stubs(:composition_tickers).returns([])
    Exchanges::Kraken.any_instance.stubs(:market_open?).returns(true)
  end

  def answer(accept, **opts) = BotApi::Bots::AnswerRedeploy.call(user: @user, bot_id: @bot.id, accept: accept, **opts)

  test 'accepting queues the redeploy and records who asked' do
    Bot::RedeployJob.expects(:perform_later).with(@bot)

    result = answer(true)

    assert result.success?, result.error_message
    assert_equal :accepted, result.status
    assert_equal true, result.data[:accepted]
    assert_equal '25.0', result.data[:offer]
    assert_equal 'redeploy_requested', @bot.bot_activity_logs.last.event
  end

  test 'a string boolean is accepted, and declining queues the decline instead' do
    Bot::RedeployJob.expects(:perform_later).with(@bot)
    assert answer('true').success?

    Bot::DeclineRedeployJob.expects(:perform_later).with(@bot, user_id: @user.id)
    result = answer(false)
    assert result.success?, result.error_message
    assert_equal false, result.data[:accepted]
  end

  test 'anything that is not a real boolean is refused, never read as a decline' do
    Bot::RedeployJob.expects(:perform_later).never
    Bot::DeclineRedeployJob.expects(:perform_later).never

    [nil, 'yes', ''].each do |value|
      assert_equal 'accept_required', answer(value).error_code, value.inspect
    end
  end

  test 'a closed market blocks accepting but not declining' do
    Exchanges::Kraken.any_instance.stubs(:market_open?).returns(false)
    assert_equal 'market_closed', answer(true).error_code

    Bot::DeclineRedeployJob.expects(:perform_later).with(@bot, user_id: @user.id)
    assert answer(false).success?
  end

  # The web button queues regardless and the jobs handle an empty offer themselves, so refusing
  # here would make the API stricter than the page for no gain.
  test 'a zero offer still queues; the job decides' do
    Bots::DcaIndex.any_instance.stubs(:redeploy_offer).returns(0.to_d)
    Bot::RedeployJob.expects(:perform_later).with(@bot)

    result = answer(true)

    assert result.success?, result.error_message
    assert_equal '0.0', result.data[:offer]
  end

  test 'a failing offer read does not stop a decline' do
    Bots::DcaIndex.any_instance.stubs(:redeploy_offer).raises(StandardError, 'cold cache')
    Bot::DeclineRedeployJob.expects(:perform_later).with(@bot, user_id: @user.id)

    result = answer(false)

    assert result.success?, result.error_message
    assert_nil result.data[:offer]
  end

  test 'only composition bots, and not archived ones' do
    single = create(:dca_single_asset, :stopped, user: @user)
    assert_equal 'not_composition_bot',
                 BotApi::Bots::AnswerRedeploy.call(user: @user, bot_id: single.id, accept: true).error_code
    @bot.update!(status: :archived)
    Bot::RedeployJob.expects(:perform_later).never
    assert_equal 'bot_archived', answer(true).error_code
  end

  test 'dry run validates and enqueues nothing' do
    Bot::RedeployJob.expects(:perform_later).never
    result = answer(true, dry_run: true)
    assert result.success?
    assert result.data[:dry_run]
    assert_equal 0, @bot.bot_activity_logs.count
  end
end
