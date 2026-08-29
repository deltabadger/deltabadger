# Converts the retired pair bot's storage shape into the general composition shape, in place.
#
# Reads pair rows through a local AR class rather than Bots::DcaDualAsset: that class is deleted in
# a later release, and a migration referencing it would stop replaying from an empty database.
# inheritance_column = nil is what lets this read and write the STI `type` column directly.
#
# Idempotent and self-repairing — safe to run repeatedly, which is how the rake task uses it.
#
# A worker cannot be stopped from here: Solid Queue is a different database, so its state cannot
# join the conversion's transaction, and a claimed job holds an already-deserialized pair instance
# regardless. So when a worker is up, a bot with a queued job is left alone.
#
# But a worker is very often NOT up. docker-entrypoint.sh runs `rails db:migrate` as its own
# process and only then execs the server, and Solid Queue runs inside Puma (config/puma.rb) — so on
# a self-hosted upgrade nothing can fire a tick while migrations run. Assuming otherwise would
# strand every WORKING pair bot on an install that has no operator to run the rake task, and the
# release that deletes Bots::DcaDualAsset would then leave rows whose class is gone —
# ActiveRecord::SubclassNotFound on instantiation, which takes the whole bot list down, not one bot.
#
# So quiescence is measured rather than assumed, from the queue's own execution tables: a bot with
# a claimed, ready, or blocked job, or a scheduled one already due, is deferred; one whose only job
# is scheduled for later is not. That converts a self-hosted install automatically at upgrade and
# still defers on a rolling deploy where the previous container is still serving.
#
# The reading is taken per bot, inside that bot's own lock, so a worker that starts mid-run defers
# every bot after it rather than none. It CANNOT rule out a worker starting between that reading
# and the commit, and no check here can: the queue lives in another database. On the standalone
# image the entrypoint rules it out anyway — migrations finish before the server is exec'd — but
# the Umbrel compose runs the worker as its own container whose `depends_on: web` waits only for
# that container to start, so there it really can register mid-migration.
#
# What makes that survivable is Bots::DcaDualAsset.find: a job holding the old GlobalID resolves to
# whatever the row is now, so a tick that fires mid-conversion runs against the basket instead of
# dead-lettering. Quiescence is the first layer, that fallback is the one that has to hold.
module Bot::DualToComposition
  class Row < ActiveRecord::Base
    self.table_name = 'bots'
    self.inheritance_column = nil
  end

  DUAL = 'Bots::DcaDualAsset'.freeze
  MULTI = 'Bots::DcaMultiAsset'.freeze
  CONDITION_TICKER_KEYS = %w[
    price_limit_in_ticker_id price_drop_limit_in_ticker_id
    moving_average_limit_in_ticker_id indicator_limit_in_ticker_id
  ].freeze
  # Running, dispatched, and waiting on the per-exchange semaphore. Scheduled is handled separately:
  # only a FUTURE scheduled execution is safe, and .due is exactly the set about to become Ready.
  BUSY_EXECUTIONS = %w[ClaimedExecution ReadyExecution BlockedExecution].freeze

  module_function

  # Is there anything at all left to do? A pair row to convert, a converted row a stale instance
  # wrote the pair shape back onto, or a queued job still naming the old class. All three have to be
  # asked: the last pair row usually disappears BEFORE the clobber that needs repairing shows up,
  # so gating on pair rows alone would switch the repair off exactly when it starts being needed.
  def pending?
    return true if Row.where(type: DUAL).exists?
    return true if clobbered.exists?

    defined?(SolidQueue) && SolidQueue::Job.where('arguments LIKE ?', "%#{DUAL}/%").exists?
  end

  # A basket a stale pair instance saved its whole settings hash over: the pair keys are there AND
  # the basket's own allocations are gone. NOT a basket that merely carries a leftover
  # base0_asset_id — a pre-#229 wizard cookie can leave one on a perfectly good basket, and
  # re-converting that would overwrite the user's allocations with the cookie's.
  def clobbered
    Row.where(type: MULTI)
       .where("json_extract(settings, '$.base0_asset_id') IS NOT NULL")
       .where("json_type(settings, '$.allocations') IS NOT 'object'")
  end

  # Safe to call repeatedly, and meant to be: a bot busy this time round converts on a later pass,
  # and the repair/sweep below run every time.
  #
  # @return [Array(Array<Integer>, Array<Array(Integer, String)>)] converted ids, [id, reason] skips
  def run!
    converted = []
    skipped = []

    Row.where(type: DUAL).find_each do |row|
      # preflight runs twice: once here for the reported reason, and again inside the lock against
      # the row as it actually is at that moment. A bot that passes here and fails there is still
      # reported — silence would read as "converted" in the migration's output.
      reason = preflight(row)
      reason = convert!(row) unless reason.is_a?(String)

      reason.is_a?(String) ? skipped << [row.id, reason] : converted << row.id
    end

    # A crash between the primary flip and the queue write — separate databases, so they cannot
    # share a transaction — leaves a converted row whose jobs still name the old class. Those rows
    # are no longer selected above, so sweep them here; this is what makes a re-run finish the job.
    sweep_stray_jobs!

    # A request that loaded the bot as a pair instance BEFORE the flip and saved AFTER it writes the
    # old settings shape onto a row that is now a basket — a lock cannot reach backwards to stop it.
    # It is cheaply detectable and the repair is this same conversion, so detect and re-run.
    repaired = repair_clobbered!
    Rails.logger.warn("dual→multi: repaired #{repaired} clobbered row(s)") if repaired.positive?

    [converted, skipped]
  end

  # @param force [Boolean] the retirement pass: nothing is refused. A gap the row cannot be
  #   converted without is recorded in the plan's :gaps instead, and convert! stops a working bot
  #   that has one.
  # @return [Hash] the conversion plan, or [String] the reason this bot is not convertible
  def preflight(row, force: false)
    settings = row.settings || {}
    base0 = settings['base0_asset_id'].to_i
    base1 = settings['base1_asset_id'].to_i
    quote = settings['quote_asset_id'].to_i
    gaps = []

    unless force
      return 'missing assets' if [base0, base1, quote].any?(&:zero?)
      return 'identical assets' if base0 == base1
      return 'missing exchange' if row.exchange_id.blank?
      return 'executing' if row.status == Bot.statuses[:executing]
      return 'rebalance in flight' if (row.transient_data || {})['rebalance_pending'].present?
      return 'job in flight' if busy_job?(row.id)
      return 'missing asset rows' unless Asset.where(id: [base0, base1, quote]).count == 3
    end

    # Float(), not to_f: to_f reads "garbage" as 0.0 and would convert the bot to a 100/0 basket.
    allocation0 = Float(settings.fetch('allocation0', 0.5), exception: false)
    unless allocation0&.finite? && allocation0.between?(0, 1)
      return 'allocation unusable' unless force

      allocation0 = 0.5
      gaps << 'allocation unusable'
    end

    # Every member the row names that has a ticker row to hang a membership on. Zero ids and a
    # repeated id collapse here, which is why the forced pass needs no separate rule for them.
    tickers = [base0, base1].uniq.reject(&:zero?)
                            .to_h { |base| [base, member_ticker(row, base, quote, force:)] }
    if tickers.size < 2 || tickers.values.any?(&:nil?)
      return 'missing ticker' unless force

      tickers.compact!
      gaps << 'missing ticker'
    end
    # A member the basket would drop at its next refresh: derive_composition renormalises the
    # survivors, and a 70/30 pair must not wake up trading as 100/0. Kept as a member, bot stopped.
    gaps << 'delisted member' if force && tickers.values.any? { |ticker| !(ticker.available? && ticker.trading_enabled?) }

    # The basket looks a condition's subject up among its own tickers, so a subject outside it is a
    # condition that can never be met. ENABLED conditions only — a disabled one carries a default
    # subject its concern wrote regardless. Kept on record, bot stopped (forced pass only).
    watched = CONDITION_TICKER_KEYS.filter_map do |key|
      next unless ActiveModel::Type::Boolean.new.cast(settings[key.sub('_in_ticker_id', 'ed')])

      settings[key].presence&.to_i
    end
    if (watched - tickers.values.map(&:id)).any?
      return 'condition watches a foreign ticker' unless force

      gaps << 'condition subject outside the basket'
    end

    # A stray membership row would break the "exactly two members" guarantee and silently add one.
    # The forced pass exits it instead (convert!).
    strays = BotIndexAsset.where(bot_id: row.id).where.not(asset_id: [base0, base1])
    return 'unexpected composition rows' if !force && strays.exists?

    { base0:, base1:, quote:, allocation0:, tickers:, gaps: }
  end

  # The basket derives from tickers that are listed and trading, so the normal pass refuses a pair
  # its venue has delisted. The forced pass reads the row without either filter: ticker rows are
  # never destroyed, and a basket tolerates an unavailable member until the user presses Start.
  def member_ticker(row, base, quote, force:)
    scope = force ? Ticker.all : Ticker.available.trading_enabled
    scope.find_by(exchange_id: row.exchange_id, base_asset_id: base, quote_asset_id: quote)
  end

  def working?(row)
    Bot.statuses.values_at(:scheduled, :executing, :retrying, :waiting).include?(row.status)
  end

  # A status check is not enough: Bot::ActionJob authenticates against the exchange and checks market
  # hours while the row still reads `scheduled` (action_job.rb:59-75), and a placement whose outcome
  # is unknown can leave no Transaction row at all (action_job.rb:138-152).
  def busy_job?(bot_id)
    return false unless defined?(SolidQueue)

    fragment = "%#{DUAL}/#{bot_id}\"%"
    busy = BUSY_EXECUTIONS.any? do |execution|
      SolidQueue.const_get(execution).joins(:job)
                .where('solid_queue_jobs.arguments LIKE ?', fragment).exists?
    end
    return true if busy

    SolidQueue::ScheduledExecution.due.joins(:job)
                                  .where('solid_queue_jobs.arguments LIKE ?', fragment).exists?
  end

  # @param from_type [String] the type the row is expected to still carry
  # @param force [Boolean] see preflight
  # @return [Array<String>] the gaps a successful conversion carried (empty when it carried none),
  #   or [String] the reason it was not converted
  def convert!(row, from_type: DUAL, force: false)
    plan = nil
    reason = nil

    ActiveRecord::Base.transaction do
      # Re-read under a row lock, then re-run preflight against THAT row. The plan preflight
      # produced before the lock describes settings a concurrent request may already have replaced,
      # and writing it would silently discard their edit — the weights and assets it carries are
      # exactly the financial settings worth losing least.
      row = Row.lock.find_by(id: row.id, type: from_type)
      if row.nil?
        reason = 'converted by a concurrent run'
        next
      end

      plan = preflight(row, force:)
      if plan.is_a?(String)
        reason = plan
        plan = nil
        next
      end

      settings = converted_settings(row, plan)
      weights = settings['allocations']
      # A derived basket's membership must be right from the first moment: conversion bypasses the
      # after_save that would normally reconcile it, and until then the bot would rebalance toward
      # stored sliders it no longer owns.
      weights = derived_weights(plan) || weights if settings['weighting'] == 'market_cap'

      # The type flips FIRST. A membership's required `bot` loads this row through STI to validate
      # it, and once the pair class is gone that load raises — so the row has to already be a
      # basket by the time the first membership is saved.
      attributes = { type: MULTI, settings: settings, updated_at: Time.current }
      # A tick that never finished: nothing revives an executing row, and retrying is what a working
      # bot is meant to be after one. Only the forced pass meets one (the normal pass refuses it).
      attributes[:status] = Bot.statuses[:retrying] if row.status == Bot.statuses[:executing]
      if plan[:gaps].any?
        # A basket that lost a member or a weight must not trade on what is left of the user's
        # intent — not on its schedule, and not through the rebalancer either, which takes stopped
        # bots with the switch on. A rebalance already in flight must not resume against it: the
        # ambiguous halt is the one state the rebalancer leaves alone, and the user resolves it
        # from the bot page.
        settings['rebalance_enabled'] = false
        attributes.merge!(status: Bot.statuses[:stopped], stopped_at: Time.current) if working?(row)
        pending = (row.transient_data || {})[Bot::Rebalanceable::PENDING_KEY]
        if pending.is_a?(Hash)
          attributes[:transient_data] = row.transient_data.merge(
            Bot::Rebalanceable::PENDING_KEY => pending.merge('phase' => Bot::Rebalanceable::PHASE_AMBIGUOUS)
          )
        end
      end
      row.update_columns(attributes)

      if force
        BotIndexAsset.where(bot_id: row.id).where.not(asset_id: plan[:tickers].keys)
                     .update_all(in_index: false, exited_at: Time.current)
      end

      weights.each do |asset_id, weight|
        ticker = plan[:tickers][asset_id.to_i]
        next if ticker.nil? # no row to hang a membership on — recorded in :gaps, bot stopped above

        membership = BotIndexAsset.find_or_initialize_by(bot_id: row.id, asset_id: asset_id.to_i)
        membership.ticker_id = ticker.id
        membership.target_allocation = weight
        membership.in_index = true
        membership.entered_at ||= row.created_at
        membership.exited_at = nil
        membership.save!
      end
    end
    return reason if plan.nil?

    # The forced pass repoints every job at the end, best-effort and in one place, so a queue-side
    # failure can never be mistaken for a failed conversion.
    repoint_queued_jobs(row.id) unless force
    plan[:gaps]
  end

  # The retirement pass. Every remaining pair row, whatever its state, because after this release
  # nothing else can: the class is gone, so a row still typed as a pair raises SubclassNotFound from
  # every bot list, and a job still naming the class can never deserialize. Nothing is refused and
  # nothing raises out — this runs from a migration, and a migration that fails is a container that
  # restarts into the same failure with no operator to help it.
  #
  # @return [Array(Array<Integer>, Array<Array(Integer, Array<String>)>, Array<Array(Integer, String)>)]
  #   converted ids; [id, gaps] for bots converted with something missing (and stopped if they were
  #   working); [id, error] for rows that raised
  def finalize!
    converted = []
    degraded = []
    failed = []

    # Pair rows, and baskets a stale pair instance wrote the pair shape back onto after Release A's
    # flip (clobbered): the recurring pass that used to repair those is deleted with this release.
    rows = Row.where(type: DUAL).map { |row| [row, DUAL] } + clobbered.map { |row| [row, MULTI] }
    rows.each do |row, from_type|
      result = convert!(row, from_type:, force: true)
      case result
      when String then converted << row.id # only 'converted by a concurrent run' is possible under force
      when [] then converted << row.id
      else degraded << [row.id, result]
      end
    rescue StandardError => e
      failed << [row.id, e.message]
      salvage!(row)
    end

    begin
      repoint_every_job!
    rescue StandardError => e
      # The primary side is done and committed; a job left naming the old class fails when a worker
      # picks it up — a scheduled tick is then re-armed by RepairOrphanedBotsJob, a one-shot stop or
      # order poll is not. Reported, not fatal.
      failed << ['queue', e.message]
    end

    [converted, degraded, failed]
  end

  # Loadable beats intact: a row whose class no longer exists raises from every bot list, not just
  # its own page. The type alone makes it a basket the user can open, stop, or delete — with settings
  # that are at least a hash, so the store accessors can read them. The rolled-back transaction also
  # took the gap safeguards with it, so they are written again here: nothing about this row is
  # trustworthy enough to rebalance on. Its own rescue, because an exception raised inside a rescue
  # clause escapes it — and the row after this one still has to be reached.
  def salvage!(row)
    row.reload
    settings = row.settings.is_a?(Hash) ? row.settings : {}
    settings = settings.except('base0_asset_id', 'base1_asset_id', 'allocation0', 'marketcap_allocated')
    transient = row.transient_data.is_a?(Hash) ? row.transient_data : {}
    pending = transient[Bot::Rebalanceable::PENDING_KEY]
    if pending.is_a?(Hash)
      transient = transient.merge(
        Bot::Rebalanceable::PENDING_KEY => pending.merge('phase' => Bot::Rebalanceable::PHASE_AMBIGUOUS)
      )
    end
    row.update_columns(type: MULTI, settings: settings.merge('rebalance_enabled' => false), transient_data: transient)
    row.update_columns(status: Bot.statuses[:stopped], stopped_at: Time.current) if working?(row)
  rescue StandardError => e
    Rails.logger.error("dual→multi: bot #{row.id} could not be salvaged: #{e.message}")
  end

  # Every queued job still naming the retired class, whatever its bot: after the forced pass there
  # is no row the old name could still be right for.
  def repoint_every_job!
    return 0 unless defined?(SolidQueue)

    SolidQueue::Job.where(finished_at: nil).where('arguments LIKE ?', "%#{DUAL}/%").find_each do |job|
      job.update_columns(arguments: rewrite_class(job.arguments))
    end
  end

  def rewrite_class(node)
    case node
    when Hash   then node.transform_values { |value| rewrite_class(value) }
    when Array  then node.map { |value| rewrite_class(value) }
    when String then node.include?("#{DUAL}/") ? node.sub("#{DUAL}/", "#{MULTI}/") : node
    else node
    end
  end

  def converted_settings(row, plan)
    settings = row.settings.dup
    %w[base0_asset_id base1_asset_id allocation0].each { |key| settings.delete(key) }
    marketcap = settings.delete('marketcap_allocated')

    # Not rounded: allocation0 and its complement sum to exactly 1 in float, and
    # bot_index_assets.target_allocation carries six decimals. Rounding here would move a weight the
    # user chose deliberately.
    settings['allocations'] = {
      plan[:base0].to_s => plan[:allocation0],
      plan[:base1].to_s => 1 - plan[:allocation0]
    }
    settings['weighting'] = marketcap.in?([true, 'true', 1, '1']) ? 'market_cap' : 'manual'
    settings
  end

  # nil when a member has no usable market cap — the caller then keeps the stored weights, which is
  # what the pair bot did. Database-only by design; see Bots::DcaMultiAsset#apply_market_cap_weights.
  def derived_weights(plan)
    caps = Asset.where(id: [plan[:base0], plan[:base1]])
                .pluck(:id, :market_cap).to_h { |id, cap| [id, cap.to_f] }
    return nil unless [plan[:base0], plan[:base1]].all? { |id| caps[id].to_f.positive? }

    Bot::Composition::Weightable.blend(market_caps: caps, flattening: 0).transform_keys(&:to_s)
  end

  # A bot finds and cancels its own queued jobs by matching the GlobalID it computes NOW
  # (schedulable.rb:132), and that string embeds the class name. After the type flip it can neither
  # see nor cancel the jobs enqueued under the old name — and those jobs would fail to deserialize,
  # since Bots::DcaDualAsset.find no longer matches the row.
  #
  # Repointing rather than deleting keeps the scheduled ActionJob's wait_until, so the bot holds its
  # exact cadence, and keeps one-shot work (a queued stop, an order poll) no repair path recreates.
  # Solid Queue is a separate database, so this cannot join the transaction above.
  def repoint_queued_jobs(bot_id)
    return 0 unless defined?(SolidQueue)

    old_gid = "#{DUAL}/#{bot_id}"
    new_gid = "#{MULTI}/#{bot_id}"

    SolidQueue::Job.where('arguments LIKE ?', "%#{old_gid}\"%").find_each do |job|
      job.update_columns(arguments: rewrite_gids(job.arguments, old_gid, new_gid))
    end
  end

  # Any queued job still addressed to the retired class whose bot has already been converted. Covers
  # an interrupted conversion, and a worker that finished a claimed job after its row was flipped.
  def sweep_stray_jobs!
    return 0 unless defined?(SolidQueue)

    converted_ids = Row.where(type: MULTI).pluck(:id).to_set
    swept = 0
    SolidQueue::Job.where('arguments LIKE ?', "%#{DUAL}/%").find_each do |job|
      id = job.arguments.to_s[%r{#{Regexp.escape(DUAL)}/(\d+)}, 1]&.to_i
      next unless id && converted_ids.include?(id)

      repoint_queued_jobs(id)
      swept += 1
    end
    swept
  end

  # A converted row carrying pair-shaped settings again was written by a stale instance after its
  # flip. Put it back through the conversion, which is idempotent and re-locks.
  #
  # The type is NOT flipped back to the pair class first: preflight can refuse the row — it may be
  # executing, or hold a live order — and that would leave a bot typed as a pair while its
  # memberships and queued GlobalIDs are already basket-shaped, which is worse than the clobber it
  # was trying to undo. convert! is told what type to expect instead, so the row's type only ever
  # changes on a conversion that succeeds.
  def repair_clobbered!
    clobbered.count { |row| !convert!(row, from_type: MULTI).is_a?(String) }
  end

  # Rebuilds rather than mutating: the parsed JSON may hold frozen strings, and sub! on one raises.
  # end_with? anchors the match so a pass for bot 7 can never rewrite bot 71's GlobalID.
  def rewrite_gids(node, old_gid, new_gid)
    case node
    when Hash   then node.transform_values { |value| rewrite_gids(value, old_gid, new_gid) }
    when Array  then node.map { |value| rewrite_gids(value, old_gid, new_gid) }
    when String then node.end_with?(old_gid) ? node.sub(old_gid, new_gid) : node
    else node
    end
  end
end
