class AccountTransactionSync
  def initialize(api_key)
    @api_key = api_key
    @exchange = api_key.exchange
  end

  def sync!(&progress)
    # Re-fetch a 25h overlap so late-posted entries (dividends post after their pay date) still land;
    # the dedup guard absorbs the repeats. A nil watermark means full history, never "since the newest
    # row we happen to hold" — otherwise a watermark reset would silently resume from truncated data.
    #
    # The floor keeps the window reaching the present. Binance, Bybit, MEXC, KuCoin and Bitget cap a
    # history query at a fixed span measured FROM start_time, so a data-derived watermark parked at a
    # quiet account's last trade would eventually query a window that ends before today — and the
    # account would go silently blind, since nothing new could arrive to advance the watermark. The
    # span is per-exchange (`Exchange#ledger_window`): Bybit's execution list and KuCoin's fills serve
    # 7 days, not the ~90 the constant was written for.
    start_time = @api_key.last_synced_at && [@api_key.last_synced_at - 25.hours, @exchange.ledger_window.ago].max
    result = @exchange.get_ledger(api_key: @api_key, start_time: start_time)
    return result if result.failure?

    entries = result.data
    total = entries.size
    imported = 0
    last_percent = 0
    min_skipped = nil

    entries.each_with_index do |entry, index|
      # A blank id identifies nothing, and several adapters produce one (Bybit's txID is empty for
      # internal transfers; Gemini/Hyperliquid/BingX build the id with .to_s on a field that can be
      # missing). Read it as nil so it dedups on the fallback identity instead of collapsing every
      # such row onto a single '' key — the partial unique index treats '' as a value.
      tx_id = entry[:tx_id].presence
      next if duplicate?(entry, tx_id)

      # Nil out zero fees — per spec, empty fields when no fee. Read into locals: `entries` belongs
      # to the adapter that returned it, and this loop must not write back into it.
      no_fee = entry[:fee_amount].blank? || entry[:fee_amount].to_d.zero?

      at = AccountTransaction.new(
        user: @api_key.user,
        api_key: @api_key,
        exchange: @exchange,
        entry_type: entry[:entry_type],
        base_currency: entry[:base_currency],
        base_amount: entry[:base_amount],
        quote_currency: entry[:quote_currency],
        quote_amount: entry[:quote_amount],
        fee_currency: no_fee ? nil : entry[:fee_currency],
        fee_amount: no_fee ? nil : entry[:fee_amount],
        tx_id: tx_id,
        group_id: entry[:group_id],
        description: entry[:description],
        transacted_at: entry[:transacted_at],
        raw_data: entry[:raw_data] || {}
      )

      match_bot_transaction!(at) if at.buy? || at.sell? || at.swap_in? || at.swap_out?

      # One malformed broker row must never abort a user's entire sync. Every exchange adapter
      # routes through this choke point, so the guard belongs here rather than in each adapter.
      # Logged at :error because that is what the fleet's healthcheck-logs scan picks up — a skipped
      # row is a hole in someone's tax history, not a curiosity.
      unless at.save
        Rails.logger.error(
          "[#{@exchange.name_id}] Account transaction sync skipped invalid entry " \
          "tx_id=#{tx_id.inspect} entry_type=#{entry[:entry_type]} " \
          "errors=#{at.errors.full_messages.join(', ')}"
        )
        min_skipped = [min_skipped, entry[:transacted_at]].compact.min
        next
      end

      stored_merged_activity_ids.merge(merged_ids_of(entry))
      imported += 1

      next unless progress && total.positive?

      percent = ((index + 1) * 100 / total)
      next unless percent != last_percent

      last_percent = percent
      progress.call(percent)
    end

    # The watermark must come from the data — Time.current silently drops anything the fetch did not
    # return. It must also never advance past a row that failed to save: that row would fall outside
    # every future window, turning one malformed entry into a permanent hole.
    max_seen = entries.filter_map { |entry| entry[:transacted_at] }.max
    watermark = [max_seen, min_skipped].compact.min
    @api_key.update!(last_synced_at: watermark || @api_key.last_synced_at, last_sync_error: nil)
    Result::Success.new(imported)
  end

  private

  def duplicate?(entry, tx_id)
    if tx_id.nil?
      # api_key too: ApiKey is unique per (user, exchange, key_type), so one user can hold two keys
      # on one exchange, and the sync jobs pass whatever key ids they are given. Without this, an
      # id-less row from the second account is swallowed by the first account's identical tuple.
      return scope.exists?(
        api_key: @api_key,
        entry_type: entry[:entry_type],
        base_currency: entry[:base_currency],
        base_amount: entry[:base_amount],
        transacted_at: entry[:transacted_at]
      )
    end

    # Only the STORED side is expanded to merged legs. A standalone leg arriving after its merged
    # adjustment is a duplicate and is skipped. The reverse is deliberately not done: skipping a
    # merged row because one of its legs is already stored would leave the ledger permanently
    # one-legged — a wrong share count that looks like data — where importing it merely double-counts
    # visibly and can be corrected.
    return true if stored_merged_activity_ids.include?(tx_id)

    scope.exists?(tx_id: tx_id)
  end

  def scope
    @scope ||= AccountTransaction.where(user: @api_key.user, exchange: @exchange)
  end

  def merged_ids_of(entry)
    raw = entry[:raw_data]
    raw.is_a?(Hash) ? Array(raw['merged_activity_ids']) : []
  end

  def stored_merged_activity_ids
    # Only rare split adjustments carry merged ids, so keep this lookup deliberately narrow.
    @stored_merged_activity_ids ||= Set.new(
      scope.where(entry_type: :adjustment).pluck(:raw_data).flat_map do |raw|
        raw.is_a?(Hash) ? Array(raw['merged_activity_ids']) : []
      end
    )
  end

  def match_bot_transaction!(at)
    return unless at.tx_id.present?

    bot_tx = Transaction.find_by(external_id: at.tx_id, exchange: @exchange)
    at.bot_transaction = bot_tx if bot_tx
  end
end
