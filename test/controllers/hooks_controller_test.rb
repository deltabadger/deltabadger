require 'test_helper'

# POST /hook/:token is the only thing that ever makes a signal bot trade. It answers with no session,
# no CSRF token and no locale — TradingView sends a bare POST whose body we never read — and it must
# answer fast (TradingView drops a webhook that takes over three seconds), so the order itself is
# placed by Bot::SignalJob, never inline.
class HooksControllerTest < ActionDispatch::IntegrationTest
  include ActiveJob::TestHelper
  include ActiveSupport::Testing::TimeHelpers

  # The app runs Solid Queue in tests too; the enqueue assertions need the test adapter.
  def queue_adapter_for_test
    ActiveJob::QueueAdapters::TestAdapter.new
  end

  setup do
    create(:user, admin: true)
    @bot = create(:signal_bot, :started)
    @signal = create(:bot_signal, bot: @bot)
  end

  teardown do
    ActionController::Base.allow_forgery_protection = false
  end

  # +body: Rack rewrites the request body's encoding in place, which a frozen string literal
  # (the default from Ruby 4 on) would not survive.
  def fire(token = @signal.token, body: 'BUY BTCUSDT', content_type: 'text/plain')
    post "/hook/#{token}", params: +body, headers: { 'CONTENT_TYPE' => content_type }
  end

  # The job carries the claim time so it can refuse a call that sat in the queue too long.
  def enqueued_for(signal)
    ->(args) { args.first(2) == [@bot, signal] && args[2].respond_to?(:to_time) }
  end

  # --- the address ---------------------------------------------------------------------------

  test 'an unknown token is not found and enqueues nothing' do
    assert_no_enqueued_jobs(only: Bot::SignalJob) { fire('not-a-token') }

    assert_response :not_found
  end

  test 'a known token on a running bot enqueues the signal job and is accepted' do
    assert_enqueued_with(job: Bot::SignalJob, args: enqueued_for(@signal)) { fire }

    assert_response :accepted
    assert_equal 'accepted', response.parsed_body['status']
  end

  test 'records when the webhook was last triggered' do
    freeze_time do
      fire

      assert_equal Time.current, @signal.reload.last_triggered_at
    end
  end

  # A webhook arrives with no session and no token; a forgery check here would 422 every real call.
  # allow_forgery_protection is off in the test environment, which would hide a controller that had
  # grown one — so it is turned back on for this test.
  test 'needs no session and no CSRF token' do
    ActionController::Base.allow_forgery_protection = true

    assert_enqueued_with(job: Bot::SignalJob) { fire }
    assert_response :accepted
  end

  # TradingView posts the alert message as text; a script may post JSON. Neither is read.
  test 'the request body is never read: any content fires the rule' do
    assert_enqueued_jobs(1, only: Bot::SignalJob) do
      fire(body: 'BUY {{ticker}} at {{close}}', content_type: 'text/plain')
    end
    assert_response :accepted

    travel BotSignal::TRIGGER_COOLDOWN + 1.second
    assert_enqueued_jobs(1, only: Bot::SignalJob) do
      fire(body: '{"ticker":"BTCUSDT","action":"buy","close":50000}', content_type: 'application/json')
    end
    assert_response :accepted
  end

  # Not even parsed: a body declared as JSON that is not JSON is not this endpoint's concern.
  test 'a body declared as JSON that is not JSON still fires the rule' do
    assert_enqueued_jobs(1, only: Bot::SignalJob) do
      fire(body: '{"action": "buy"', content_type: 'application/json')
    end

    assert_response :accepted
  end

  test 'a token in the body cannot redirect the call to another rule' do
    other = create(:bot_signal, bot: @bot)

    assert_enqueued_with(job: Bot::SignalJob, args: enqueued_for(@signal)) do
      fire(body: { token: other.token }.to_json, content_type: 'application/json')
    end
  end

  # A GET is what a link preview, a browser prefetch or a curious click sends. None of them may trade.
  test 'is only routed as a POST' do
    assert_no_enqueued_jobs(only: Bot::SignalJob) { get "/hook/#{@signal.token}" }

    assert_response :not_found
    assert_nil @signal.reload.last_triggered_at
  end

  # The request line of the Rails log is not covered by filter_parameters; a path segment is
  # logged verbatim unless the request masks it itself.
  test 'the token never reaches the request log' do
    request = ActionDispatch::Request.new(Rack::MockRequest.env_for("/hook/#{@signal.token}", method: 'POST'))

    assert_no_match @signal.token, request.filtered_path
    assert_match %r{\A/hook/\[FILTERED\]}, request.filtered_path
  end

  test 'lives outside the locale scope' do
    assert_no_enqueued_jobs(only: Bot::SignalJob) { post "/en/hook/#{@signal.token}" }

    assert_response :not_found
  end

  # --- replay -------------------------------------------------------------------------------

  # Acknowledged, not refused: a sender that treats non-2xx as a failed delivery (TradingView does)
  # must not see a retry it asked for reported as a failure.
  test 'a second call inside the cooldown is acknowledged, ignored and enqueues nothing' do
    fire

    assert_no_enqueued_jobs(only: Bot::SignalJob) { fire }
    assert_response :ok
    assert_equal({ 'status' => 'ignored', 'reason' => 'cooldown' }, response.parsed_body)
  end

  test 'fires again once the cooldown has passed' do
    freeze_time do
      fire
      travel BotSignal::TRIGGER_COOLDOWN

      assert_enqueued_with(job: Bot::SignalJob, args: enqueued_for(@signal)) { fire }
      assert_response :accepted
    end
  end

  test 'rules on one bot cool down independently' do
    other = create(:bot_signal, bot: @bot)
    fire

    assert_enqueued_with(job: Bot::SignalJob, args: enqueued_for(other)) { fire(other.token) }
    assert_response :accepted
  end

  # The queue lives in its own SQLite file, so the claim and the enqueue can never be one
  # transaction. When the queue refuses the row — Solid Queue raises its own EnqueueError, which
  # Active Job does NOT rescue into `false` — the claim has to be handed back, or the sender's
  # immediate retry would be swallowed as a cooldown replay of a call that was never kept.
  test 'a call the queue would not keep is refused and the claim released' do
    Bot::SignalJob.stubs(:perform_later).raises(SolidQueue::Job::EnqueueError, 'database is locked')

    fire
    assert_response :service_unavailable
    assert_nil @signal.reload.last_triggered_at

    Bot::SignalJob.unstub(:perform_later)
    assert_enqueued_with(job: Bot::SignalJob, args: enqueued_for(@signal)) { fire }
    assert_response :accepted
  end

  test 'a quietly refused enqueue is treated the same way' do
    Bot::SignalJob.stubs(:perform_later).returns(false)

    fire

    assert_response :service_unavailable
    assert_nil @signal.reload.last_triggered_at
  end

  test 'releasing the claim restores the timestamp the call found' do
    earlier = 10.minutes.ago.change(usec: 0)
    @signal.update!(last_triggered_at: earlier)
    Bot::SignalJob.stubs(:perform_later).returns(false)

    fire

    assert_response :service_unavailable
    assert_equal earlier, @signal.reload.last_triggered_at
  end

  # Request A stalls in the queue long enough for the cooldown to lapse and request B to claim
  # and enqueue. A's release must not hand B's claim back too, or a third call trades at once.
  test 'releasing the claim never erases a newer claim' do
    newer = nil
    Bot::SignalJob.stubs(:perform_later).with do
      newer = 31.seconds.from_now.change(usec: 0)
      BotSignal.where(id: @signal.id).update_all(last_triggered_at: newer)
      true
    end.raises(SolidQueue::Job::EnqueueError, 'database is locked')

    fire

    assert_response :service_unavailable
    assert_equal newer, @signal.reload.last_triggered_at
  end

  # --- a call that cannot run ---------------------------------------------------------------

  test 'a disabled rule is acknowledged and ignored' do
    @signal.update!(enabled: false)

    assert_no_enqueued_jobs(only: Bot::SignalJob) { fire }
    assert_response :ok
    assert_equal({ 'status' => 'ignored', 'reason' => 'signal_disabled' }, response.parsed_body)
    assert_predicate @signal.reload.last_triggered_at, :present?
  end

  test 'a stopped bot is acknowledged and ignored' do
    @bot.update!(status: :stopped)

    assert_no_enqueued_jobs(only: Bot::SignalJob) { fire }
    assert_response :ok
    assert_equal({ 'status' => 'ignored', 'reason' => 'bot_not_running' }, response.parsed_body)
  end

  test 'an archived bot is acknowledged and ignored' do
    @bot.update!(status: :archived)

    assert_no_enqueued_jobs(only: Bot::SignalJob) { fire }
    assert_response :ok
    assert_equal 'bot_not_running', response.parsed_body['reason']
  end

  test 'a deleted bot is not found' do
    @bot.delete

    assert_no_enqueued_jobs(only: Bot::SignalJob) { fire }
    assert_response :not_found
  end

  # A TradingView alert against a stopped bot can call every 30 seconds for weeks. The widget's
  # "last triggered" line is where that shows; the activity feed is not.
  test 'an ignored call writes no activity row' do
    @bot.update!(status: :stopped)

    assert_no_difference('BotActivityLog.count') { fire }
  end
end
