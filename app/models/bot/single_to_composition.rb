# Converts a single-asset bot into a one-asset multi-asset bot, in place: the same row, id, history,
# schedule and settings, with the pair's asset as the one member at 100%.
#
# Reads and writes rows through local AR classes rather than the bot models: the single-asset class is
# deleted in a later release, and a migration that references it would stop replaying from an empty
# database. inheritance_column = nil is what lets this read and write the STI `type` column directly.
#
# Idempotent and self-repairing — the migration runs it once at boot and Bot::ConvertSingleAssetBotsJob
# runs it again until nothing is left.
#
# Quiescence is measured, not assumed (Bot::ConversionQueue.busy?): a bot with a claimed, ready or
# blocked job, or a scheduled one already due, is deferred to a later pass; one whose only job is
# scheduled for later converts, and the job is repointed at the new class. At migrate time no worker runs
# on the standalone image (the entrypoint migrates before Puma, which hosts Solid Queue), so what defers
# a bot there is a row left from the last shutdown; the recurring pass converts it once a worker has
# drained it.
#
# The conversion refuses — and reports — what it cannot carry over unchanged: a bot mid-tick, a condition
# watching another ticker, a composition row of another asset, and histories the two bots read differently
# (orders of another asset or none, non-regular orders, a sale executed without reported proceeds). A
# working bot on a ticker its venue no longer trades waits too: the single-asset bot kept placing there,
# the multi-asset one stops before placement — a change a running bot should not get silently.
module Bot::SingleToComposition
  class Row < ActiveRecord::Base
    self.table_name = 'bots'
    self.inheritance_column = nil
  end

  # Memberships through a local class too: BotIndexAsset's `belongs_to :bot` loads the bot through STI to
  # validate it, and the migration must not depend on future model code.
  class Membership < ActiveRecord::Base
    self.table_name = 'bot_index_assets'
  end

  SINGLE = 'Bots::DcaSingleAsset'.freeze
  MULTI = 'Bots::DcaMultiAsset'.freeze
  # Every condition a bot can watch a ticker with, buy and sell side. Each is keyed `<name>_in_ticker_id`
  # and switched by `<name>ed`.
  CONDITIONS = %w[price_limit price_drop_limit moving_average_limit indicator_limit]
               .flat_map { |name| [name, "sell_#{name}"] }.freeze

  module_function

  # Anything left for the recurring pass: a single-asset row, a clobbered basket, or a job still naming
  # the single-asset class.
  def pending?
    Row.where(type: SINGLE).exists? || clobbered.exists? || Bot::ConversionQueue.names?(SINGLE)
  end

  # A basket a stale single-asset instance saved its whole settings hash over: the pair's key is back AND
  # the basket's allocations are gone. A cell that is not JSON is skipped rather than raising the scan.
  def clobbered
    Row.where(type: MULTI)
       .where("CASE WHEN json_valid(settings) THEN json_extract(settings, '$.base_asset_id') END IS NOT NULL")
       .where("CASE WHEN json_valid(settings) THEN json_type(settings, '$.allocations') END IS NOT 'object'")
  end

  # @return [Array(Array<Integer>, Array<Array(Integer, String)>)] converted ids, [id, reason] skips
  def run!
    converted = []
    skipped = []

    Row.where(type: SINGLE).find_each do |row|
      # Run twice: here for the reported reason, and again inside the lock against the row as it is then.
      reason = preflight(row)
      reason = convert!(row) unless reason.is_a?(String)
      reason.is_a?(String) ? skipped << [row.id, reason] : converted << row.id
    rescue StandardError => e
      # A migration that raises is a self-hosted container restarting into the same failure.
      skipped << [row.id, "error: #{e.message}"]
    end

    begin
      Bot::ConversionQueue.sweep!(from: SINGLE, to: MULTI)
    rescue StandardError => e
      skipped << ['queue', "error: #{e.message}"]
    end
    repair_clobbered!(skipped)

    [converted, skipped]
  end

  # @param repair [Boolean] the row is a clobbered basket: jobs naming either class hold it back
  # @return [Hash] the conversion plan, or [String] the reason this bot is not converted now
  def preflight(row, repair: false)
    settings = row.settings.is_a?(Hash) ? row.settings : {}
    base = settings['base_asset_id'].to_i
    quote = settings['quote_asset_id'].to_i
    return 'missing assets' if base.zero? || quote.zero?
    return 'missing exchange' if row.exchange_id.blank?
    return 'missing asset rows' unless Asset.where(id: [base, quote]).count == 2

    # No availability filter: the membership hangs on the ticker row whatever its state.
    ticker = Ticker.find_by(exchange_id: row.exchange_id, base_asset_id: base, quote_asset_id: quote)
    return 'missing ticker' if ticker.nil?
    return 'executing' if row.status == Bot.statuses[:executing]
    return 'job in flight' if busy_job?(row.id, repair ? [SINGLE, MULTI] : [SINGLE])
    return 'ticker not tradeable' if working?(row) && !(ticker.available? && ticker.trading_enabled?)

    # The basket looks a condition's subject up among its own tickers, so a subject outside it is a
    # condition that can never be met. Enabled conditions only: a disabled one carries whatever default
    # subject its concern wrote.
    watched = CONDITIONS.filter_map do |name|
      settings["#{name}_in_ticker_id"].presence&.to_i if ActiveModel::Type::Boolean.new.cast(settings["#{name}ed"])
    end
    return 'condition watches another ticker' if (watched - [ticker.id]).any?
    # A repair meets the membership its own conversion wrote, which a stale save may have moved away from:
    # the single-asset bot refuses an asset change once it has orders, so such a bot has none to lose, and
    # the membership is exited instead (convert!).
    return 'unexpected composition rows' if !repair && Membership.where(bot_id: row.id).where.not(asset_id: base).exists?

    divergence = history_divergence(row.id, base)
    return divergence if divergence

    { base:, ticker: }
  end

  # The histories the two walks read differently, so the figures would change. Rows under a renamed or
  # legacy symbol are not among them: both walks read the asset, whatever the row recorded.
  def history_divergence(bot_id, base)
    rows = Transaction.where(bot_id:, status: Transaction.statuses[:submitted])
    return 'orders of another asset' if rows.where('base_asset_id IS NULL OR base_asset_id != ?', base).exists?
    return 'non-regular orders' if rows.where.not(transaction_type: 'REGULAR').exists?

    unpriced = rows.where(side: Transaction.sides[:sell])
                   .where('quote_amount_exec IS NULL OR quote_amount_exec <= 0')
                   .where('COALESCE(amount_exec, CASE WHEN external_status = ? THEN amount END) > 0',
                          Transaction.external_statuses[:closed])
    'unpriced sell' if unpriced.exists?
  end

  def working?(row)
    Bot.statuses.values_at(:scheduled, :retrying, :waiting).include?(row.status)
  end

  def busy_job?(bot_id, classes)
    Bot::ConversionQueue.busy?(classes.map { |klass| "#{klass}/#{bot_id}" })
  end

  # @param from_type [String] the type the row is expected to still carry (MULTI for a clobber repair)
  # @return [nil] converted, or [String] the reason it was not
  def convert!(row, from_type: SINGLE)
    plan = nil
    reason = nil

    ActiveRecord::Base.transaction do
      # Re-read under a row lock, then re-run preflight against THAT row: a request may have changed it
      # since, and the plan must describe the row being written.
      row = Row.lock.find_by(id: row.id, type: from_type)
      if row.nil?
        reason = 'converted by a concurrent run'
        next
      end
      if from_type == MULTI && !clobbered.exists?(id: row.id)
        reason = 'repaired by a save'
        next
      end

      plan = preflight(row, repair: from_type == MULTI)
      if plan.is_a?(String)
        reason = plan
        plan = nil
        next
      end

      settings = row.settings.except('base_asset_id')
      settings['allocations'] = { plan[:base].to_s => 1.0 }
      # The single-asset bot read a blank sell sentence as a base amount, the basket reads it as quote.
      settings['sell_denomination'] = 'base' if settings['sell_denomination'].blank?
      # The type flips FIRST, then the membership. update_columns, never save: a save would bump
      # settings_changed_at and trip the carry guard, and run every callback of a class being retired.
      row.update_columns(type: MULTI, settings:, updated_at: Time.current)

      Membership.where(bot_id: row.id, in_index: true).where.not(asset_id: plan[:base])
                .update_all(in_index: false, exited_at: Time.current, updated_at: Time.current)
      membership = Membership.find_or_initialize_by(bot_id: row.id, asset_id: plan[:base])
      membership.ticker_id = plan[:ticker].id
      membership.target_allocation = 1.0
      membership.in_index = true
      membership.entered_at ||= row.created_at
      membership.exited_at = nil
      membership.save! if membership.changed?
    end
    return reason if plan.nil?

    Bot::ConversionQueue.repoint(row.id, from: SINGLE, to: MULTI)
    nil
  end

  # A converted row carrying the single-asset shape again was written by a stale instance after its flip.
  # Put it back through the conversion, told to expect a basket, so its type only ever changes forward.
  def repair_clobbered!(skipped)
    clobbered.find_each do |row|
      reason = convert!(row, from_type: MULTI)
      skipped << [row.id, "not repaired: #{reason}"] if reason && reason != 'repaired by a save'
    rescue StandardError => e
      skipped << [row.id, "error: #{e.message}"]
    end
  rescue StandardError => e
    skipped << ['repair', "error: #{e.message}"]
  end
end
