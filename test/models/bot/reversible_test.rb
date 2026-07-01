require 'test_helper'

# M1 — Direction foundation + manual flip (see plan: reverse a bot into selling).
# These cover the Bot::Reversible concern only (the controller #reverse action and the
# claimed-job deferral guard are a separate batch).
class Bot::ReversibleTest < ActiveSupport::TestCase
  include ActiveSupport::Testing::TimeHelpers

  # == Defaults & predicates (reader fallback — no persisted-on-load default) ==

  test 'a new bot defaults to buying' do
    bot = build(:dca_single_asset)
    assert_equal 'buying', bot.direction
    assert_predicate bot, :buying?
    assert_not_predicate bot, :selling?
  end

  test 'the buying default is never written into settings (existing-row safe)' do
    # An old row created before this feature has no "direction" key. Reading the default
    # must not add the key, or it would persist a default into settings on save.
    bot = create(:dca_single_asset)
    assert_not bot.settings.key?('direction'),
               'creating a bot must not persist "direction" into settings'

    bot.reload
    assert_equal 'buying', bot.direction
    assert_not bot.settings.key?('direction'),
               'reading the default must not write "direction" into settings'
  end

  test 'selling? is true once direction is selling' do
    bot = build(:dca_single_asset)
    bot.direction = 'selling'
    assert_predicate bot, :selling?
    assert_not_predicate bot, :buying?
  end

  # == Validation (inclusion, allow_nil so existing rows never fail) ==

  test 'rejects an unknown direction' do
    bot = build(:dca_single_asset)
    bot.direction = 'sideways'
    assert_not_predicate bot, :valid?
    assert bot.errors[:direction].present?
  end

  test 'accepts selling as a direction' do
    bot = build(:dca_single_asset)
    bot.direction = 'selling'
    assert_predicate bot, :valid?
  end

  test 'a nil stored direction is valid and reads as buying' do
    bot = build(:dca_single_asset)
    bot.direction = nil
    assert_predicate bot, :valid?
    assert_equal 'buying', bot.direction
  end

  # sell_interval feeds effective_interval_duration → next_interval_checkpoint_at. An
  # unsupported value would make the duration nil and crash scheduling on the flip, so reject it
  # at save time (a tampered request can't persist it while buying and wedge a later reverse).
  test 'rejects an unsupported sell_interval' do
    bot = build(:dca_single_asset)
    bot.sell_interval = 'fortnight'
    assert_not_predicate bot, :valid?
    assert bot.errors[:sell_interval].present?
  end

  test 'accepts a supported sell_interval' do
    bot = build(:dca_single_asset)
    bot.sell_interval = 'week'
    assert_predicate bot, :valid?
  end

  test 'a blank stored sell_interval is valid (reader falls back to the buy interval)' do
    bot = build(:dca_single_asset)
    assert_predicate bot, :valid?
    assert_equal bot.interval, bot.sell_interval
  end

  # == flip_direction! ==

  test 'flip_direction! switches buying to selling and persists' do
    bot = create(:dca_single_asset, :started)
    assert_predicate bot, :buying?

    bot.flip_direction!

    assert_predicate bot, :selling?
    assert_predicate bot.reload, :selling?
  end

  test 'flip_direction! switches selling back to buying' do
    bot = create(:dca_single_asset, :started)
    bot.direction = 'selling'
    bot.set_missed_quote_amount
    bot.save!

    bot.flip_direction!

    assert_predicate bot.reload, :buying?
  end

  test 'flip_direction! on a working bot saves without raising the Accountable guard' do
    bot = create(:dca_single_asset, :started)
    assert_nothing_raised { bot.flip_direction! }
    assert_predicate bot.reload, :selling?
  end

  test 'flip_direction! resets the schedule anchor (started_at) to the flip time' do
    bot = create(:dca_single_asset, :started)
    bot.update_column(:started_at, 3.days.ago)

    freeze_time do
      bot.flip_direction!
      assert_in_delta Time.current.to_f, bot.reload.started_at.to_f, 1.0
    end
  end

  test 'flip_direction! schedules a fresh action job (no stranding)' do
    bot = create(:dca_single_asset, :started)
    SolidQueue::Job.destroy_all
    SolidQueue::ScheduledExecution.destroy_all

    bot.flip_direction!

    assert bot.reload.next_action_job_at.present?,
           'flip must schedule a fresh Bot::ActionJob so the bot is not stranded'
  end

  # Reversing must NOT start an inactive bot — it only changes the stored direction, so the bot
  # runs the new way when the user next starts it (no trade scheduled outside the start lifecycle).
  test 'flip_direction! on a stopped bot only changes direction and does not start it' do
    bot = create(:dca_single_asset, :stopped)
    SolidQueue::Job.destroy_all

    bot.flip_direction!

    assert_predicate bot.reload, :selling?
    assert_equal 'stopped', bot.status
    assert_not SolidQueue::Job.where(class_name: 'Bot::ActionJob').exists?,
               'reversing a stopped bot must not enqueue an ActionJob'
  end

  test 'flip_direction! on a created (never-started) bot only changes direction' do
    bot = create(:dca_single_asset) # status :created
    SolidQueue::Job.destroy_all

    bot.flip_direction!

    assert_predicate bot.reload, :selling?
    assert_equal 'created', bot.status
    assert_not SolidQueue::Job.where(class_name: 'Bot::ActionJob').exists?
  end

  test 'flip_direction! cancels a pending limit-check job so no stale resume races the fresh action job' do
    bot = create(:dca_single_asset, :waiting)
    bot.log_activity('limit_paused', details: { limit_type: :price })
    Bot::PriceLimitCheckJob.set(wait_until: 1.minute.from_now).perform_later(bot)
    assert SolidQueue::Job.where(class_name: 'Bot::PriceLimitCheckJob').exists?,
           'precondition: a limit-check job is scheduled'

    bot.flip_direction!

    assert_not SolidQueue::Job.where(class_name: 'Bot::PriceLimitCheckJob').exists?,
               'flip must cancel the pending limit-check job (else it would resume the old direction)'
  end

  test 'flip_direction! cancels a BLOCKED old-direction action job (concurrency-limited, not just scheduled)' do
    bot = create(:dca_single_asset, :started)
    Bot::ActionJob.perform_later(bot)
    job = SolidQueue::Job.where(class_name: 'Bot::ActionJob').last
    # Move the job into BlockedExecution (as a per-exchange concurrency limit would).
    SolidQueue::ReadyExecution.where(job_id: job.id).delete_all
    SolidQueue::ScheduledExecution.where(job_id: job.id).delete_all
    SolidQueue::BlockedExecution.create!(job_id: job.id, queue_name: job.queue_name,
                                         priority: job.priority, concurrency_key: "bot_#{bot.id}",
                                         expires_at: 5.minutes.from_now)

    bot.flip_direction!

    assert_not SolidQueue::Job.exists?(job.id),
               'a blocked old-direction job must be cancelled by the flip, not left to unblock later'
  end

  # == flip_blocked_by_inflight_job? (manual-flip deferral guard) ==
  # A claimed (mid-run) job cannot be cancelled by flip_direction! step 3, so the manual path
  # (controller) must defer when one is in flight. Trigger flips (M5) run INSIDE the claimed
  # ActionJob and bypass this guard by calling flip_direction! directly.

  test 'flip_blocked_by_inflight_job? is false when nothing is claimed' do
    bot = create(:dca_single_asset, :started)
    assert_not bot.flip_blocked_by_inflight_job?
  end

  test 'flip_blocked_by_inflight_job? is true when a Bot::ActionJob is claimed for this bot' do
    bot = create(:dca_single_asset, :started)
    Bot::ActionJob.perform_later(bot)
    job_id = SolidQueue::Job.where(class_name: 'Bot::ActionJob').last.id
    process = SolidQueue::Process.create!(kind: 'Worker', pid: 1, name: 'test-worker', last_heartbeat_at: Time.current)
    SolidQueue::ReadyExecution.where(job_id:).delete_all
    SolidQueue::ClaimedExecution.create!(job_id:, process_id: process.id)

    assert bot.flip_blocked_by_inflight_job?
  end

  test 'flip_blocked_by_inflight_job? is true when a limit-check job is claimed for this bot' do
    bot = create(:dca_single_asset, :waiting)
    Bot::PriceLimitCheckJob.perform_later(bot)
    job_id = SolidQueue::Job.where(class_name: 'Bot::PriceLimitCheckJob').last.id
    process = SolidQueue::Process.create!(kind: 'Worker', pid: 1, name: 'test-worker', last_heartbeat_at: Time.current)
    SolidQueue::ReadyExecution.where(job_id:).delete_all
    SolidQueue::ClaimedExecution.create!(job_id:, process_id: process.id)

    assert bot.flip_blocked_by_inflight_job?
  end

  test 'flip_blocked_by_inflight_job? ignores claimed jobs belonging to another bot' do
    bot = create(:dca_single_asset, :started)
    other = create(:dca_single_asset, :started, user: bot.user, exchange: bot.exchange,
                                                base_asset: bot.base_asset, quote_asset: bot.quote_asset)
    Bot::ActionJob.perform_later(other)
    job_id = SolidQueue::Job.where(class_name: 'Bot::ActionJob').last.id
    process = SolidQueue::Process.create!(kind: 'Worker', pid: 1, name: 'test-worker', last_heartbeat_at: Time.current)
    SolidQueue::ReadyExecution.where(job_id:).delete_all
    SolidQueue::ClaimedExecution.create!(job_id:, process_id: process.id)

    assert_not bot.flip_blocked_by_inflight_job?
  end

  # == Cancel unfilled orders on reversal ==
  # Reversing must cancel the bot's still-open orders so they don't linger on the wrong side and so
  # an unfilled buy can't be mistaken for accumulated holdings after the flip.

  test 'flip_direction! cancels the unfilled orders, leaving the filled ones alone' do
    bot = create(:dca_single_asset, :started)
    bot.set_missed_quote_amount
    bot.save!
    create(:transaction, bot: bot, side: :buy, status: :submitted, external_status: :open,
                         external_id: 'open1', amount: 1, quote_amount: 100)
    create(:transaction, bot: bot, side: :buy, status: :submitted, external_status: :closed,
                         external_id: 'closed1', amount: 1, amount_exec: 1, quote_amount: 100)
    # Only the single waiting order is cancelled (the closed one is not in the waiting scope).
    Transaction.any_instance.expects(:cancel).once.returns(Result::Success.new)

    bot.flip_direction!

    assert_predicate bot.reload, :selling?
  end

  test 'flip_direction! cancels nothing when there are no open orders' do
    bot = create(:dca_single_asset, :started)
    bot.set_missed_quote_amount
    bot.save!
    Transaction.any_instance.expects(:cancel).never

    assert_nothing_raised { bot.flip_direction! }
  end

  test 'cancel_unfilled_orders returns the reserve of successfully-cancelled BUY orders only' do
    bot = create(:dca_single_asset, :started)
    bot.set_missed_quote_amount
    bot.save!
    create(:transaction, bot: bot, side: :buy, status: :submitted, external_status: :open,
                         external_id: 'b1', amount: 0.4, price: 100, quote_amount: 40, amount_exec: nil, quote_amount_exec: nil)
    create(:transaction, bot: bot, side: :sell, status: :submitted, external_status: :open,
                         external_id: 's1', amount: 1, price: 100, quote_amount: 100, amount_exec: nil, quote_amount_exec: nil)
    Transaction.any_instance.stubs(:cancel).returns(Result::Success.new)

    assert_in_delta 40, bot.send(:cancel_unfilled_orders).to_f, 1e-6, 'only the cancelled buy counts'
  end

  test 'a failed buy cancel is not counted in the restored reserve (order may still be live)' do
    bot = create(:dca_single_asset, :started)
    bot.set_missed_quote_amount
    bot.save!
    create(:transaction, bot: bot, side: :buy, status: :submitted, external_status: :open,
                         external_id: 'b1', amount: 0.4, price: 100, quote_amount: 40, amount_exec: nil, quote_amount_exec: nil)
    Transaction.any_instance.stubs(:cancel).returns(Result::Failure.new('still live'))

    assert_in_delta 0, bot.send(:cancel_unfilled_orders).to_f, 1e-6
  end

  test 'flip_direction! restores the cancelled buy reserve to the deflated carry (capped at one tranche)' do
    bot = create(:dca_single_asset, :started) # effective_quote_amount 100
    create(:transaction, bot: bot, side: :buy, status: :submitted, external_status: :open,
                         external_id: 'b1', amount: 0.4, price: 100, quote_amount: 40, amount_exec: nil, quote_amount_exec: nil)
    Transaction.any_instance.stubs(:cancel).returns(Result::Success.new)
    # The open buy deflated the carry to 60 (set_missed reads pending); the cancelled 40 is restored.
    bot.stubs(:pending_quote_amount).returns(60.to_d)

    bot.flip_direction!

    assert_predicate bot.reload, :selling?
    assert_in_delta 100, bot.missed_quote_amount.to_f, 1e-6 # 60 + 40, capped at effective 100
  end

  test 'a failed cancel during a flip is recorded as an activity event (not just a log line)' do
    bot = create(:dca_single_asset, :started)
    bot.set_missed_quote_amount
    bot.save!
    create(:transaction, bot: bot, side: :buy, status: :submitted, external_status: :open,
                         external_id: 'open1', amount: 1, quote_amount: 100)
    Transaction.any_instance.stubs(:cancel).returns(Result::Failure.new('exchange said no'))

    bot.flip_direction!

    assert BotActivityLog.where(bot_id: bot.id, event: 'order_cancel_failed').exists?,
           'a dangling order left by a failed cancel must be visible in the activity feed'
  end

  # == sell_amount validation ==

  test 'a blank sell_amount stays valid (not yet configured)' do
    bot = create(:dca_single_asset)
    bot.direction = 'selling'
    bot.sell_amount = nil

    assert_predicate bot, :valid?
  end

  test 'a negative or zero sell_amount is rejected' do
    bot = create(:dca_single_asset)
    bot.direction = 'selling'

    bot.sell_amount = -1
    assert_not bot.valid?

    bot.sell_amount = 0
    assert_not bot.valid?
  end

  # == clearing the sell amount (blank = the no-op state) ==

  test 'submitting a blank sell_amount clears it (does not keep the stale value)' do
    bot = create(:dca_single_asset)
    parsed = bot.parse_params(ActionController::Parameters.new(sell_amount: '').permit!)

    assert parsed.key?(:sell_amount), 'a submitted-but-blank sell amount must be an explicit clear'
    assert_nil parsed[:sell_amount]
  end

  test 'omitting the sell_amount field leaves it untouched' do
    bot = create(:dca_single_asset)
    parsed = bot.parse_params(ActionController::Parameters.new(quote_amount: '100').permit!)

    assert_not parsed.key?(:sell_amount), 'a form that did not submit sell_amount must not clear it'
  end
end
