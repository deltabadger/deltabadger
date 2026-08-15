class AccountTransactionSync
  def initialize(api_key)
    @api_key = api_key
    @exchange = api_key.exchange
  end

  def sync!(&progress)
    # Re-fetch a 25h overlap so late-posted entries (dividends post after their pay date) still land;
    # the dedup guard absorbs the repeats. A nil watermark means full history, never "since the newest
    # row we happen to hold" — otherwise a watermark reset would silently resume from truncated data.
    start_time = @api_key.last_synced_at && (@api_key.last_synced_at - 25.hours)
    result = @exchange.get_ledger(api_key: @api_key, start_time: start_time)
    return result if result.failure?

    entries = result.data
    total = entries.size
    imported = 0
    last_percent = 0

    entries.each_with_index do |entry, index|
      # A blank id identifies nothing, and several adapters produce one (Bybit's txID is empty for
      # internal transfers; Gemini/Hyperliquid/BingX build the id with .to_s on a field that can be
      # missing). Normalise it to nil so it dedups on the fallback identity instead of collapsing
      # every such row onto a single '' key — the partial unique index treats '' as a value.
      entry[:tx_id] = entry[:tx_id].presence
      next if duplicate?(entry)

      # Nil out zero fees — per spec, empty fields when no fee
      if entry[:fee_amount].blank? || entry[:fee_amount].to_d.zero?
        entry[:fee_amount] = nil
        entry[:fee_currency] = nil
      end

      at = AccountTransaction.new(
        user: @api_key.user,
        api_key: @api_key,
        exchange: @exchange,
        entry_type: entry[:entry_type],
        base_currency: entry[:base_currency],
        base_amount: entry[:base_amount],
        quote_currency: entry[:quote_currency],
        quote_amount: entry[:quote_amount],
        fee_currency: entry[:fee_currency],
        fee_amount: entry[:fee_amount],
        tx_id: entry[:tx_id],
        group_id: entry[:group_id],
        description: entry[:description],
        transacted_at: entry[:transacted_at],
        raw_data: entry[:raw_data] || {}
      )

      match_bot_transaction!(at) if at.buy? || at.sell? || at.swap_in? || at.swap_out?

      # One malformed broker row must never abort a user's entire sync. Every exchange adapter
      # routes through this choke point, so the guard belongs here rather than in each adapter.
      unless at.save
        Rails.logger.warn(
          "[#{@exchange.name_id}] Account transaction sync skipped invalid entry " \
          "tx_id=#{entry[:tx_id].inspect} entry_type=#{entry[:entry_type]} " \
          "errors=#{at.errors.full_messages.join(', ')}"
        )
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

    # The watermark must come from the data — Time.current silently drops anything the fetch did not return.
    max_seen = entries.filter_map { |entry| entry[:transacted_at] }.max
    @api_key.update!(last_synced_at: max_seen || @api_key.last_synced_at)
    Result::Success.new(imported)
  end

  private

  def duplicate?(entry)
    ids = [entry[:tx_id], *merged_ids_of(entry)].compact
    if ids.empty?
      return scope.exists?(
        entry_type: entry[:entry_type],
        base_currency: entry[:base_currency],
        base_amount: entry[:base_amount],
        transacted_at: entry[:transacted_at]
      )
    end

    return true if ids.any? { |id| stored_merged_activity_ids.include?(id) }

    scope.exists?(tx_id: ids)
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
