class Bot::ActionJob < BotJob
  # Retries exhausted. Keep the bot in :retrying and reschedule a fresh attempt at the next
  # interval. Transient (network / -1021 timestamp) exhaustion is self-recovering: log a calm,
  # de-emphasized (:info) entry and DON'T email "your bot failed". Everything else (incl.
  # rate-limit exhaustion) stays red (:error) + notifies. NOTE: activity rows aren't currently
  # color-coded by level, so the user-visible lever here is the suppressed email; :info keeps it
  # gray, not yellow, if level styling is added later.
  EXHAUSTION_HANDLER = lambda do |job, error, exhausted_detail|
    bot = job.arguments.first
    next unless Bot::ActionJob.transition_working_bot!(bot, 'retrying')

    if exhausted_detail[:transient_exhausted]
      bot.record_failure!(:transient)
      bot.log_activity('execution_retrying', level: :info,
                                             details: { error: error.message }.merge(exhausted_detail))
    else
      kind = bot.exchange&.failure_kind([error.message])
      notify = bot.notify_about_failure?(kind)
      bot.record_failure!(kind, notified: notify)
      bot.log_activity('execution_failed', level: :error,
                                           details: { error: error.message, kind: kind }.merge(exhausted_detail))
      bot.notify_about_error(errors: Bot::ActionJob.humanized_errors(bot, error.message)) if notify
    end
    Bot::ActionJob.set(wait_until: bot.next_interval_checkpoint_at).perform_later(bot)
    Bot::BroadcastAfterScheduledActionJob.perform_later(bot)
  end

  # Humanize for an EMAIL, in the recipient's language. humanize_error resolves I18n.t eagerly and a
  # job thread carries no request locale, so translating here and handing the sentence to
  # BotAlertsMailer — which only calls set_locale afterwards — put English error text inside an
  # otherwise translated email.
  def self.humanized_errors(bot, message)
    I18n.with_locale(bot.user&.locale || I18n.default_locale) do
      [bot.exchange&.humanize_error(message) || message]
    end
  end

  retry_on Client::TransientNetworkError,
           wait: :polynomially_longer,
           attempts: 4 do |job, error|
    EXHAUSTION_HANDLER.call(job, error, transient_exhausted: true)
  end

  # Rate limits retry on their own longer, escalating wait (BotJob::RATE_LIMIT_WAIT) so we
  # don't keep re-tripping the exchange's decaying counter.
  retry_on Client::RateLimitedError,
           wait: BotJob::RATE_LIMIT_WAIT,
           attempts: 4 do |job, error|
    EXHAUSTION_HANDLER.call(job, error, rate_limited_exhausted: true)
  end

  # A stop or delete from another process (user click, admin stock deactivation sweep) can land
  # while this job is mid-flight on a stale bot instance; Stop's cancel can't reach a Claimed
  # (running) execution, and a check-then-update! would leave a resurrection window. The
  # conditional UPDATE closes it: the flip only happens while the row is still in a working
  # status — a stop, delete or archive that won the race leaves it out of scope — and the caller
  # branches on the outcome. Skipped callbacks are covered on every call site: the status
  # bar is (re)broadcast by BroadcastAfterScheduledActionJob or explicitly, and the
  # button/columns-lock UI does not differ between working statuses.
  def self.transition_working_bot!(bot, status)
    updated = Bot.where(id: bot.id).working
                 .update_all(status: status, updated_at: Time.current) == 1
    # Sync the attribute without reload — reload would drop memoized associations; only the
    # status column changed, and no later save runs on these paths.
    bot.status = status if updated
    updated
  end

  def perform(bot)
    action_started_at = Time.current
    return unless bot.scheduled? || bot.retrying?

    # Another ActionJob is already queued for this bot, so THIS one is the duplicate. Return without
    # touching anything: raising used to flip a perfectly healthy bot to :retrying on its way to the
    # dead-letter, and now that the rescue reschedules instead of re-raising it would also queue a
    # second successor and keep the duplicate chain alive.
    if bot.next_action_job_at.present?
      Rails.logger.warn("ActionJob for bot #{bot.id}: an action job is already scheduled, skipping this duplicate")
      return
    end

    # An IBKR key registered but not yet activated by IBKR (24h–2wk). Reschedule WITHOUT touching
    # the exchange — a pending key must never reach a live IBKR call. Ibkr::CheckActivationJob flips
    # the key to :correct on activation, and the next run proceeds.
    if bot.api_key&.pending_activation?
      Rails.logger.info("ActionJob for bot #{bot.id}: api_key pending IBKR activation, rescheduling")
      schedule_next_action_job(bot)
      return
    end

    bot.ensure_exchange_authenticated
    bot_tickers = bot.tickers.to_a
    unless bot.exchange.market_open?(tickers: bot_tickers)
      Rails.logger.info("ActionJob for bot #{bot.id}: market closed, rescheduling to #{bot.exchange.next_market_open_at(tickers: bot_tickers)}")
      bot.update!(waiting_for_market_open: true)
      bot.log_activity('market_closed', details: { next_market_open_at: bot.exchange.next_market_open_at(tickers: bot_tickers) })
      Bot::ActionJob.set(wait_until: bot.exchange.next_market_open_at(tickers: bot_tickers)).perform_later(bot)
      Bot::BroadcastAfterScheduledActionJob.perform_later(bot)
      return
    end

    # Market confirmed open: clear any stale market-closed flag immediately, bypassing validation
    # (Fix C). Otherwise a separate problem — e.g. a temporarily-unavailable ticker that makes the
    # success-path update! below raise — would leave the flag stuck and the UI showing "market
    # closed" for a non-market issue.
    clear_stale_market_closed_flag(bot)

    bot.update!(last_action_job_at: Time.current, waiting_for_market_open: nil)
    result = bot.execute_action
    if result.failure?
      Rails.logger.error("ActionJob for bot #{bot.id} failed to execute action. Errors: #{result.errors.to_sentence}")
      # A -1021/timestamp rejection on placement is a definitive pre-trade rejection (the order was
      # never placed — see Exchange#placement_transient_error?). It is self-recovering: reschedule a
      # fresh attempt at the next interval (clean — no orphan), log a calm gray entry, and send NO
      # alarm email. Everything else stays the existing red path (raise → rescue StandardError).
      if bot.exchange.placement_transient_error?(result.errors)
        return unless self.class.transition_working_bot!(bot, 'retrying')

        # Records a kind even though nothing is notified here: Bot::Failable's strike is "the same
        # blocking kind TWICE IN A ROW", so every failing exit has to write what it was. Skip this
        # and invalid_key -> timestamp rejection -> invalid_key would stop the bot, on the strength
        # of two failures that were not consecutive.
        bot.record_failure!(:transient)
        bot.log_activity('execution_retrying', level: :info,
                                               details: { error: result.errors.to_sentence, placement_transient: true })
        schedule_next_action_job(bot)
        return
      end
      raise result.errors.to_sentence
    end

    # A clean run ends the streak. The notification budget deliberately survives it — see
    # Bot::Failable — so a bot that fails, recovers and fails the same way an hour later does not
    # mail twice.
    bot.clear_failure_state!

    # The starting-time feature only affects the FIRST execution; flip it off
    # afterwards so the rule UI is free to be reconfigured for a future restart.
    bot.disable_starting_time! if bot.respond_to?(:disable_starting_time!) && bot.start_time_enabled?

    if result.data.present? && result.data[:break_reschedule]
      Rails.logger.info("ActionJob for bot #{bot.id} reschedule disabled.")
      bot.log_activity('reschedule_disabled')
    elsif self.class.transition_working_bot!(bot, 'scheduled')
      # No broadcast here on purpose — BroadcastAfterScheduledActionJob handles it after the
      # next job is actually scheduled.
      schedule_next_action_job(bot)
    else
      Rails.logger.info("ActionJob for bot #{bot.id}: bot was stopped mid-execution, leaving it stopped")
    end
  rescue Client::AmbiguousPlacementError => e
    # The order MAY be live on the exchange — the request timed out after it was sent, and
    # placement carries no idempotency key. Re-raising would hand this to `retry_on
    # Client::TransientNetworkError` (if it were transient) or dead-letter the job; either way the
    # placement must never be attempted again for this tick, because a replay buys twice.
    # Latent, not observed: the 24 close-together duplicates found on 2026-07-27 traced to
    # user-initiated restarts, not to this path.
    #
    # So: no re-raise, no re-place. Record it visibly and let the next interval proceed normally.
    # Deliberately NOT logged as execution_failed — we do not know that it failed, and saying so
    # would be a lie.
    #
    # KNOWN GAP, second half: because no Transaction is written, Bot::Accountable#pending_quote_amount
    # counts this tick as MISSED and carries its amount into the next order — so a landed-but-unseen
    # 10 followed by a normal 20 buys 30 across two ticks. Not reserved here on purpose: reserving
    # assumes the order landed, not reserving assumes it did not, and each is wrong half the time.
    # A Transaction row cannot express "unknown" either — the enum is submitted/failed/skipped, and
    # there is no external_id to reconcile against later. Client order ids fix this properly; a
    # coin-flip reservation would just move the error.
    #
    # KNOWN GAP (pre-existing, not introduced here): if the order DID land, nothing reconciles it.
    # Bots::*::OrderSetter#set_order only calls persist_accepted_order! on a successful result, so
    # a raised placement leaves no Transaction and no external_id, and
    # Bot::FetchAndUpdateOpenOrdersJob polls only ids drawn from bot.transactions.waiting. A landed
    # limit order can therefore rest untracked. This code path still strictly reduces the harm —
    # before it, the retry placed up to three MORE untracked orders — but closing the gap needs
    # exchange-history recovery keyed on the placement window, which is deliberately not in this
    # change.
    Rails.logger.warn("ActionJob for bot #{bot.id}: placement outcome unknown, not retrying. #{e.message}")
    return unless self.class.transition_working_bot!(bot, 'retrying')

    bot.record_failure!(nil)
    bot.log_activity('placement_ambiguous', level: :warning, details: { error: e.message })
    # NOTE: start_time_enabled is deliberately left as-is here, and in the post-placement branch
    # below. Disarming it would be more correct when a leg was already accepted (a date-mode
    # start_at then sits in the past and the user must reconfigure before restarting) — but
    # disable_starting_time! ends in save!, and at this point transition_working_bot! has set
    # bot.status in memory WITHOUT saving it. A stop landing in that window would be overwritten by
    # the save, resurrecting a bot the user just stopped. The success path avoids this by disarming
    # BEFORE the transition; here the transition must come first, because it is how we detect the
    # stop. Trading a stopped-bot resurrection for a start-time tidy-up is a bad deal, so the
    # residual stands: after an ambiguous or suppressed tick a one-shot start may need reconfiguring.
    schedule_next_action_job(bot)
  rescue Client::TransientNetworkError, Client::RateLimitedError => e
    # An order ALREADY PLACED during this perform makes the retry unsafe: replaying re-runs
    # execute_action end to end, placing a second order for the same tick.
    #
    # The reachable path: Bot::Fundable#execute_action calls `super` (which places the order) and
    # THEN funds_are_low? -> get_balance, a live network read. A raise there escapes with the order
    # already live, and retry_on replays it. That particular raise needs an APP-level client
    # (Alpaca, IBKR); the honeymaker gem returns Result::Failure for network errors and never raises.
    #
    # But do NOT read that as "honeymaker venues are safe". The app converts those Results back into
    # raises wherever it wants the retry chain — dca_single_asset/order_setter.rb:88 and :176, plus
    # the fetch jobs — for EVERY venue. Those four sites happen to be pre-placement today, which is
    # the only reason the replay is not already armed fleet-wide. Moving any transient_error? raise
    # to after create_order re-arms it on all 13 honeymaker venues at once.
    #
    # DEFENSIVE, NOT A POSTMORTEM FIX. The 24 duplicate placements found on 2026-07-27 were
    # investigated and attributed to user-initiated restarts, and 14 of them predate retry_on
    # landing in this job. This guard has not been observed preventing a real duplicate; it is here
    # because the replay is reachable and costs real money if it ever fires.
    #
    # Scope limit worth knowing: this is INTRA-PERFORM. It keys on action_started_at, a local of
    # this execution, so it cannot see a placement made by a DIFFERENT job. Two separately enqueued
    # ActionJobs each placing one order are not covered here.
    #
    # No activity row and no email here on purpose: from the user's side nothing failed. The order
    # was placed and is polled as usual; only a follow-up step blipped.
    # Scoped to :submitted — the only status meaning "sent and accepted by the exchange". A
    # :skipped row (a below-minimum leg on an index/dual bot) or a :failed one (exchange rejected
    # it) leaves no live order, so neither may suppress a safe retry.
    if bot.transactions.submitted.where('created_at >= ?', action_started_at).exists?
      Rails.logger.warn(
        "ActionJob for bot #{bot.id}: #{e.class} after an order was already placed this tick — " \
        "not retrying, to avoid a duplicate order. #{e.message}"
      )
      return unless self.class.transition_working_bot!(bot, 'retrying')

      bot.record_failure!(:transient)
      # start_time_enabled is left as-is for the resurrection reason documented on the
      # ambiguous-placement branch above: disable_starting_time! saves, and bot.status is dirty
      # here because transition_working_bot! wrote it via update_all without persisting the model.
      schedule_next_action_job(bot)
      return
    end

    # Nothing placed yet: replaying is safe, and it is what carries bots through exchange-proxy
    # blips. Skip the noisy execution_failed path. Leaving the bot in :retrying ensures the
    # ActiveJob retry chain (and any post-exhaustion reschedule) passes the line-8 guard on the next
    # perform.
    if self.class.transition_working_bot!(bot, 'retrying')
      bot.record_failure!(:transient) # breaks a blocking streak — see the placement branch above
      bot.broadcast_status_bar_update
    end
    raise
  rescue StandardError => e
    Rails.logger.error("ActionJob for bot #{bot.id} failed to perform. Errors: #{e.message}")
    unless self.class.transition_working_bot!(bot, 'retrying')
      Rails.logger.info("ActionJob for bot #{bot.id}: bot was stopped mid-execution, skipping retry handling")
      return
    end

    bot.broadcast_status_bar_update

    kind = bot.exchange&.failure_kind([e.message])
    blocking = bot.blocking_failure?(kind)
    # Read the budget BEFORE writing the row that would answer it.
    notify = blocking || bot.notify_about_failure?(kind)
    bot.record_failure!(kind, notified: notify)

    # A failed order already records its own Transaction row; only log execution_failed
    # for failures that left no transaction (auth, market/API, unexpected errors).
    unless bot.transactions.failed.where('created_at >= ?', action_started_at).exists?
      bot.log_activity('execution_failed', level: :error, details: { error: e.message, kind: kind })
    end

    if blocking
      stop_for_blocking_failure(bot, kind, e)
    else
      notify_recoverable(bot, kind, e) if notify
      schedule_next_action_job(bot)
    end
  end

  private

  def clear_stale_market_closed_flag(bot)
    return unless bot.waiting_for_market_open

    bot.merge_transient_data!('waiting_for_market_open' => nil)
  end

  # The venue is refusing the credentials, a scope, or the asset itself, and has now done so on two
  # consecutive runs. Nothing here recovers on a schedule, so the bot is stopped rather than left
  # failing daily and mailing about it.
  #
  # perform_now, not perform_later: perform_later would leave the bot :retrying until a worker picks
  # the job up, and Bot::RepairOrphanedBotsJob only looks for a missing ActionJob — so a delayed or
  # failed stop leaves a window in which the sweep re-arms the bot and it trades again. It also lets
  # the "we stopped your bot" mail overtake the stop itself, since mailers share a worker group with
  # :default. Stopping inline closes both: perform does not return until the row says :stopped.
  #
  # On a FRESHLY loaded bot, because transition_working_bot! above left `status` dirty in memory via
  # update_all, and a save from that instance is the documented resurrection hazard. A new instance
  # cannot carry that dirt, which is a stronger guarantee than ordering the writes carefully.
  #
  # Through Bot::StopJob rather than Bot#stop, because the job also rebroadcasts the settings and
  # exchange-select panels; an inline stop leaves them rendering the running state. It raises if the
  # stop fails, and that raise propagates — loud, no mail, bot still :retrying, next sweep retries.
  def stop_for_blocking_failure(bot, kind, error)
    # Re-read and re-check: a stop, archive or (soft) delete from a web request can land between the
    # transition above and here, and the user who just deleted this bot should not be told we
    # stopped it. Bot#stop refuses to write over :archived/:deleted anyway — this keeps the mail
    # honest too.
    fresh = Bot.find(bot.id)
    return unless fresh.working?

    Bot::StopJob.perform_now(fresh, stop_message_key: "bot.status.stopped_by_error.#{kind}")
    bot.notify_stopped_by_error(errors: self.class.humanized_errors(bot, error.message))
  end

  # notify_end_of_funds always names the QUOTE asset (Bot::Notifyable), but a selling bot spends
  # base — so for a sell it would tell the user to top up the asset that did not run out.
  # Bot::Fundable skips its own low-funds check while selling for exactly this reason. The
  # classification still stands (don't retry-storm a balance rejection); only the wording changes.
  def notify_recoverable(bot, kind, error)
    if kind == :insufficient_funds && !bot.selling?
      bot.notify_end_of_funds
    else
      bot.notify_about_error(errors: self.class.humanized_errors(bot, error.message))
    end
  end

  def schedule_next_action_job(bot)
    Bot::ActionJob.set(wait_until: bot.next_interval_checkpoint_at).perform_later(bot)
    Bot::BroadcastAfterScheduledActionJob.perform_later(bot)
  end
end
