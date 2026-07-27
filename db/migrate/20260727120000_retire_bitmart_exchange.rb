# BitMart shut down and honeymaker 0.10.0 dropped the venue. Two branches, decided by whether
# anything still points at the exchange: a clean install (every production container — a fleet scan
# found zero references of any kind) loses the row and its ~900 tickers outright, while an install
# holding Bitmart history keeps them, unavailable, so the bots stay visible, their trade records
# (tax records) survive, and the user can move them to a live exchange.
#
# Raw SQL throughout: the app models keep changing, and Exchanges::Bitmart is now a stub whose
# STI row this migration may be about to delete.
class RetireBitmartExchange < ActiveRecord::Migration[8.1]
  RETIRED_TYPE = 'Exchanges::Bitmart'.freeze

  # Automation::Statusable: %i[created scheduled stopped deleted executing retrying waiting]
  STOPPED = 2
  DELETED = 3
  # Transaction#external_status: %i[unknown open closed cancelled abandoned]
  EXTERNAL_UNKNOWN = 0
  EXTERNAL_OPEN = 1
  EXTERNAL_ABANDONED = 4
  # Transaction#status: %i[submitted failed skipped]
  SUBMITTED = 0

  def up
    prune_index_exchange_maps

    exchange_id = select_value("SELECT id FROM exchanges WHERE type = '#{RETIRED_TYPE}'")
    return if exchange_id.blank?

    abandon_unresolvable_orders(exchange_id)
    remove_api_keys_and_balances(exchange_id)
    affected_bot_gids = bot_global_ids(exchange_id)
    stop_automations(exchange_id)
    cancel_queued_jobs(affected_bot_gids)

    if references?(exchange_id)
      retain_as_untradeable(exchange_id)
    else
      delete_exchange(exchange_id)
    end
  end

  def down
    raise ActiveRecord::IrreversibleMigration
  end

  private

  # Both maps are JSON keyed by exchange type. Done in Ruby rather than SQLite's json_remove so the
  # key stays a literal (it contains '::') and the rewrite is obvious.
  def prune_index_exchange_maps
    rows = select_all('SELECT id, available_exchanges, top_coins_by_exchange FROM indices').to_a
    rows.each do |row|
      available = parse_json_object(row['available_exchanges'])
      top_coins = parse_json_object(row['top_coins_by_exchange'])
      next unless available.key?(RETIRED_TYPE) || top_coins.key?(RETIRED_TYPE)

      available.delete(RETIRED_TYPE)
      top_coins.delete(RETIRED_TYPE)
      execute(<<~SQL.squish)
        UPDATE indices
        SET available_exchanges = #{quote(available.to_json)},
            top_coins_by_exchange = #{quote(top_coins.to_json)}
        WHERE id = #{row['id'].to_i}
      SQL
    end
  end

  # An order left :open or :unknown would be polled forever against a venue that cannot answer, and
  # transactions.waiting is exactly what validate_unchangeable_exchange blocks the exchange switch
  # on. :abandoned is the state Bot::StaleOrderResolver already uses for "the exchange no longer
  # tracks this order". `status` is untouched — the trade record itself does not change.
  def abandon_unresolvable_orders(exchange_id)
    execute(<<~SQL.squish)
      UPDATE transactions
      SET external_status = #{EXTERNAL_ABANDONED}, updated_at = CURRENT_TIMESTAMP
      WHERE exchange_id = #{exchange_id}
        AND status = #{SUBMITTED}
        AND external_status IN (#{EXTERNAL_UNKNOWN}, #{EXTERNAL_OPEN})
    SQL
  end

  # Deleting the keys is what makes a stranded bot read as "not connected" in the UI. The nullify
  # first is required: account_transactions.api_key_id carries an FK.
  #
  # account_balances go with them, on BOTH branches. They are refreshable snapshots, not history —
  # AccountBalance::SyncJob rebuilds them from a :correct trading key, and that key is gone one line
  # above. Left behind on the retain branch they would freeze forever, and TrackerController's
  # portfolio sums every nonzero balance across all exchanges with no availability filter, so the
  # user would keep seeing phantom BitMart holdings in their totals.
  def remove_api_keys_and_balances(exchange_id)
    execute("UPDATE account_transactions SET api_key_id = NULL WHERE api_key_id IN " \
            "(SELECT id FROM api_keys WHERE exchange_id = #{exchange_id})")
    execute("DELETE FROM api_keys WHERE exchange_id = #{exchange_id}")
    execute("DELETE FROM fee_api_keys WHERE exchange_id = #{exchange_id}")
    execute("DELETE FROM account_balances WHERE exchange_id = #{exchange_id}")
  end

  # NOT IN (stopped, deleted) rather than != stopped: a deleted bot must not be resurrected as a
  # stopped one. Rules take the status only — the rules table has no stop_message_key column.
  def stop_automations(exchange_id)
    execute(<<~SQL.squish)
      UPDATE bots
      SET status = #{STOPPED}, stopped_at = CURRENT_TIMESTAMP, updated_at = CURRENT_TIMESTAMP,
          stop_message_key = 'bot.status.exchange_retired'
      WHERE exchange_id = #{exchange_id} AND status NOT IN (#{STOPPED}, #{DELETED})
    SQL
    execute(<<~SQL.squish)
      UPDATE rules
      SET status = #{STOPPED}, updated_at = CURRENT_TIMESTAMP
      WHERE exchange_id = #{exchange_id} AND status NOT IN (#{STOPPED}, #{DELETED})
    SQL
  end

  # Collected BEFORE the bots are stopped, so the set is the same either way.
  def bot_global_ids(exchange_id)
    select_all("SELECT id, type FROM bots WHERE exchange_id = #{exchange_id}").to_a.map do |row|
      "gid://#{GlobalID.app}/#{row['type']}/#{row['id']}"
    end
  end

  # Stopping a bot with raw SQL cannot cancel its queued Solid Queue jobs the way
  # Bot::Lifecycle#stop → cancel_scheduled_action_jobs does — and that helper only cancels the
  # ACTION job anyway, so a delayed Bot::*LimitCheckJob (queue :default) survives a stop AND a
  # later start. Left behind, one could fire after the user has moved the bot to a live exchange
  # and, since the bot is waiting again, hand off to a second ActionJob chain — the duplicate-order
  # shape this codebase has been bitten by before. The same sweep clears the legacy
  # Bot::FetchAndCreateOrderJob shim, which resolves its venue through the mutable bot association
  # and would otherwise query the NEW exchange with an old Bitmart order id.
  #
  # Best-effort by design: a self-hosted install may run a different queue backend, and failing to
  # tidy the queue must never wedge the migration (and with it, container start-up).
  def cancel_queued_jobs(bot_gids)
    return if bot_gids.empty?
    return unless defined?(SolidQueue)

    [SolidQueue::ScheduledExecution, SolidQueue::ReadyExecution, SolidQueue::BlockedExecution].each do |execution_class|
      execution_class.joins(:job).find_each do |execution|
        execution.job.destroy if job_references_bot?(execution.job, bot_gids)
      end
    end
  rescue StandardError => e
    # Loud on purpose: this sweep is what stops a surviving legacy Bot::FetchAndCreateOrderJob from
    # resolving its venue through the (by then possibly switched) bot association. If it ever fails,
    # the operator needs to see it in the deploy log rather than find it later.
    say "WARNING: could not clear queued jobs for retired bots (#{e.class}: #{e.message}). " \
        "Inspect the queue for jobs referencing #{bot_gids.join(', ')} before restarting those bots."
    Rails.logger.error("[RetireBitmartExchange] queue cleanup failed: #{e.class}: #{e.message}")
  end

  # Mirrors Automation::Schedulable#job_matches_record? — ActiveJob serializes a record argument as
  # { "_aj_globalid" => "gid://…" }.
  def job_references_bot?(job, bot_gids)
    return false if job&.arguments.blank?

    args = job.arguments.is_a?(Hash) ? job.arguments['arguments'] : job.arguments
    return false unless args.is_a?(Array)

    args.any? do |arg|
      gid = arg.is_a?(Hash) ? arg['_aj_globalid'] : arg
      gid.is_a?(String) && bot_gids.include?(gid)
    end
  end

  # Every FK into the exchange, plus the one indirect route: bot_index_assets.ticker_id is NOT NULL
  # with an FK, so a stale row left by an index bot that moved to another venue would make the
  # ticker delete fail. Any hit means retention.
  # account_balances and fee_api_keys are absent from this list on purpose: both are deleted above,
  # so they can never be a reason to retain.
  def references?(exchange_id)
    direct = %w[bots transactions account_transactions rules].any? do |table|
      select_value("SELECT 1 FROM #{table} WHERE exchange_id = #{exchange_id} LIMIT 1").present?
    end
    return true if direct

    select_value(<<~SQL.squish).present?
      SELECT 1 FROM bot_index_assets
      WHERE ticker_id IN (SELECT id FROM tickers WHERE exchange_id = #{exchange_id})
      LIMIT 1
    SQL
  end

  def retain_as_untradeable(exchange_id)
    execute("UPDATE exchanges SET available = 0, updated_at = CURRENT_TIMESTAMP WHERE id = #{exchange_id}")
    execute("UPDATE tickers SET available = 0, trading_enabled = 0, updated_at = CURRENT_TIMESTAMP " \
            "WHERE exchange_id = #{exchange_id}")
  end

  def delete_exchange(exchange_id)
    execute("DELETE FROM exchange_assets WHERE exchange_id = #{exchange_id}")
    execute("DELETE FROM tickers WHERE exchange_id = #{exchange_id}")
    execute("DELETE FROM exchanges WHERE id = #{exchange_id}")
  end

  def parse_json_object(value)
    parsed = value.is_a?(String) ? JSON.parse(value.presence || '{}') : value
    parsed.is_a?(Hash) ? parsed : {}
  rescue JSON::ParserError
    {}
  end

  def quote(value)
    ActiveRecord::Base.connection.quote(value)
  end
end
