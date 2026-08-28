# Converts the retired pair bot's storage shape into the general composition shape, in place.
#
# Reads pair rows through a local AR class rather than Bots::DcaDualAsset: that class is deleted in
# a later release, and a migration referencing it would stop replaying from an empty database.
# inheritance_column = nil is what lets this read and write the STI `type` column directly.
#
# Idempotent and self-repairing — safe to run repeatedly, which is how the rake task uses it.
#
# KNOWN RESIDUAL: a job claimed by a worker in the moment between the in-lock preflight and the type
# flip is not stopped by any of this. Solid Queue is a different database, so its state cannot join
# the transaction, and the worker holds an already-deserialized pair instance either way. The window
# is microseconds and every busy state is refused before it, but closing it properly needs an
# execution fence (a generation counter on the bot, re-checked before a job's final writes) — a
# schema change, deliberately not bolted on here. Converting with bot job processing drained is the
# way to avoid it entirely, which is what the rake task exists for.
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

  # @return [Hash] the conversion plan, or [String] the reason this bot is not convertible
  def preflight(row)
    settings = row.settings || {}
    base0 = settings['base0_asset_id'].to_i
    base1 = settings['base1_asset_id'].to_i
    quote = settings['quote_asset_id'].to_i

    return 'missing assets' if [base0, base1, quote].any?(&:zero?)
    return 'identical assets' if base0 == base1
    return 'missing exchange' if row.exchange_id.blank?
    return 'executing' if row.status == Bot.statuses[:executing]
    return 'rebalance in flight' if (row.transient_data || {})['rebalance_pending'].present?
    return 'live order' if Transaction.where(bot_id: row.id).waiting.exists?
    return 'job in flight' if busy_job?(row.id)
    return 'missing asset rows' unless Asset.where(id: [base0, base1, quote]).count == 3

    # Float(), not to_f: to_f reads "garbage" as 0.0 and would convert the bot to a 100/0 basket.
    allocation0 = Float(settings.fetch('allocation0', 0.5), exception: false)
    return 'allocation unusable' unless allocation0&.finite? && allocation0.between?(0, 1)

    # available.trading_enabled, matching derive_composition: a ticker the target class would refuse
    # to derive from must not become a membership row.
    tickers = [base0, base1].to_h do |base|
      [base, Ticker.available.trading_enabled
                   .find_by(exchange_id: row.exchange_id, base_asset_id: base, quote_asset_id: quote)]
    end
    return 'missing ticker' if tickers.values.any?(&:nil?)

    watched = CONDITION_TICKER_KEYS.filter_map { |key| settings[key].presence&.to_i }
    return 'condition watches a foreign ticker' unless (watched - tickers.values.map(&:id)).empty?

    # A stray membership row would break the "exactly two members" guarantee and silently add one.
    return 'unexpected composition rows' if BotIndexAsset.where(bot_id: row.id)
                                                         .where.not(asset_id: [base0, base1]).exists?

    { base0:, base1:, quote:, allocation0:, tickers: }
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
  # @return [true] on success, or [String] the reason it was not converted
  def convert!(row, from_type: DUAL)
    settings = nil
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

      plan = preflight(row)
      if plan.is_a?(String)
        reason = plan
        next
      end

      settings = converted_settings(row, plan)
      weights = settings['allocations']
      # A derived basket's membership must be right from the first moment: conversion bypasses the
      # after_save that would normally reconcile it, and until then the bot would rebalance toward
      # stored sliders it no longer owns.
      weights = derived_weights(plan) || weights if settings['weighting'] == 'market_cap'

      weights.each do |asset_id, weight|
        membership = BotIndexAsset.find_or_initialize_by(bot_id: row.id, asset_id: asset_id.to_i)
        membership.ticker_id = plan[:tickers][asset_id.to_i].id
        membership.target_allocation = weight
        membership.in_index = true
        membership.entered_at ||= row.created_at
        membership.exited_at = nil
        membership.save!
      end

      row.update_columns(type: MULTI, settings: settings, updated_at: Time.current)
    end
    return reason if settings.nil?

    repoint_queued_jobs(row.id)
    true
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
    victims = Row.where(type: MULTI).where("json_extract(settings, '$.base0_asset_id') IS NOT NULL")
    victims.count { |row| convert!(row, from_type: MULTI) == true }
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
