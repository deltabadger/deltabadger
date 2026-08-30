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
    # On a venue that caps the returned window (`Exchange#ledger_window`), the start is floored so the
    # window still reaches the present: those endpoints measure the cap FROM start_time, so a
    # data-derived watermark parked at a quiet account's last trade would query a window that ends
    # before today, and the account would go silently blind since nothing new could arrive to advance
    # the watermark. An uncapped venue gets NO floor — clamping there would simply drop the months
    # between two tracker visits.
    floor = @exchange.ledger_window&.ago
    start_time = @api_key.last_synced_at && [@api_key.last_synced_at - 25.hours, floor].compact.max
    result = @exchange.get_ledger(api_key: @api_key, start_time: start_time)
    return result if result.failure?

    entries = result.data
    outcome = store!(entries, &progress)

    # The watermark must come from the data — Time.current silently drops anything the fetch did not
    # return. It must also never advance past a row that failed to save: that row would fall outside
    # every future window, turning one malformed entry into a permanent hole.
    max_seen = entries.filter_map { |entry| entry[:transacted_at] }.max
    watermark = [max_seen, outcome[:min_skipped]].compact.min
    @api_key.update!(last_synced_at: watermark || @api_key.last_synced_at, last_sync_error: nil)
    Result::Success.new(outcome[:imported])
  end

  # Writes ledger entries for ONE key: dedup, build, match a bot's own order, save. The sync feeds
  # it what the venue returned; the importer feeds it what a file said.
  #
  # Both go through here on purpose. A file overlaps the window the API already covered, and the
  # dedup below is what keeps the overlap from landing twice — a second copy of this logic anywhere
  # else would drift from it, and the drift would only ever show up as a doubled tax history.
  #
  # The WATERMARK stays with the caller: an import must never advance `last_synced_at`, or the next
  # sync would skip the window the file happened to reach.
  def store!(entries, &progress)
    total = entries.size
    imported = 0
    duplicates = 0
    skipped = 0
    last_percent = 0
    min_skipped = nil

    @file_pairs = entries.select { |entry| entry[:group_id].to_s.start_with?('swapcsv_') }.group_by { |entry| entry[:group_id] }

    entries.each_with_index do |entry, index|
      # A blank id identifies nothing, and several adapters produce one (Bybit's txID is empty for
      # internal transfers; Gemini/Hyperliquid/BingX build the id with .to_s on a field that can be
      # missing). Read it as nil so it dedups on the fallback identity instead of collapsing every
      # such row onto a single '' key — the partial unique index treats '' as a value.
      tx_id = entry[:tx_id].presence
      if duplicate?(entry, tx_id)
        duplicates += 1
        next
      end

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
        skipped += 1
        next
      end

      stored_merged_activity_ids.merge(merged_ids_of(entry))
      log_split(at)
      imported += 1

      next unless progress && total.positive?

      percent = ((index + 1) * 100 / total)
      next unless percent != last_percent

      last_percent = percent
      progress.call(percent)
    end

    { imported: imported, duplicates: duplicates, skipped: skipped, min_skipped: min_skipped }
  end

  # Every bot whose walk would fold this event in — that is, one that traded the symbol on this
  # venue. Deliberately NOT `bots_holding`: that filters to a non-flat position, which is a rule
  # about who should be told, not about whose numbers move.
  def self.expire_restated_bots(user:, exchange:, symbol:)
    return if symbol.blank?

    Bot.where(id: Transaction.submitted.where(base: symbol, exchange: exchange)
                             .where(bot_id: Bot.where(user: user).select(:id))
                             .select(:bot_id))
       .find_each do |bot|
      bot.expire_restated_metrics!
      # Dropping the caches fixes the next render; a page already open would go on showing the
      # pre-split position until someone reloaded it. Enqueued rather than rendered here — the
      # partial reads live prices, and a ledger sync is not the place to make that call.
      Bot::BroadcastMetricsUpdateJob.perform_later(bot)
    end
  end

  # One info line per bot that was holding the symbol when it was restated. Also called from the
  # migration that backfills the splits already on record, which is why it takes plain arguments
  # and no sync state.
  #
  # A split OLDER than the feed's retention is not written at all: `BotActivityLog::PruneJob` would
  # delete it within the day, and by then the trades it exists to explain are gone from the feed
  # too. Dated at the split rather than at the sync, so it ages out alongside them.
  def self.announce_split(user:, exchange:, symbol:, at:, ratio: nil)
    return if symbol.blank? || at.blank? || at < BotActivityLog::PruneJob::RETENTION.ago

    details = { base: symbol, ratio: ratio }.compact
    bots_holding(user, exchange, symbol, at).each do |bot|
      # Alpaca can ship the add leg on its own and the merged pair a sync later — one split
      # arriving twice, the second time knowing the ratio. Upgrade the line rather than add a
      # second one saying the same thing with less in it.
      #
      # Matched on the SYMBOL too, in Ruby: two holdings can split on the same day and Alpaca
      # books both at that day's midnight, so the timestamp alone would read the second as a
      # repeat of the first and quietly overwrite it.
      existing = bot.bot_activity_logs.where(event: 'asset_split', created_at: at)
                    .find { |log| log.details['base'] == symbol }
      next existing.update(details: details) if existing

      bot.log_activity('asset_split', at: at, details: details)
    end
  end

  # A bot is affected if it still had the symbol when it was restated. Read through
  # `confirmed_exec_amounts`, the app's one rule for what an order actually moved: an order that
  # failed or was skipped moved nothing, an accepted one that never filled moved nothing either,
  # and one that filled part way before being cancelled moved exactly what it filled.
  #
  # Suppressed only on a position that reads EXACTLY flat. A bot's own rows are as-traded and are
  # never restated, so once a symbol has been through an earlier split the buys and the sells are
  # in different units and the sum is not a share count — it can even go negative. A number that
  # cannot be a position is not evidence the bot has none; a clean zero is. This errs toward
  # saying too much, which for one informational line is the right direction, and it stops mattering
  # once the ledger itself is restated.
  def self.bots_holding(user, exchange, symbol, at)
    # Scoped by the TRANSACTION's venue, not the bot's current one: a bot can be moved between
    # brokers and its old orders keep the exchange they were placed on, so reading the bot's
    # present venue would count a position it never held here — and hide one it did.
    # `created_at` is acceptance, not fill, and `transactions` records no execution time. It is
    # nonetheless the right side of the split for every row that can matter here: Alpaca places
    # stock orders `time_in_force: 'day'` (see Exchanges::Alpaca), a day order cannot survive the
    # session close, and a split is booked at the day boundary — so acceptance and fill are always
    # on the same side of one. Crypto orders are GTC and can span midnight, but crypto has no
    # corporate actions. Revisit if stock orders ever become GTC.
    rows = Transaction.submitted.where(base: symbol, exchange: exchange)
                      .where(created_at: ...at)
                      .where(bot_id: Bot.where(user: user).select(:id))
                      .pluck(:bot_id, :side, :external_status, :price, :amount, :amount_exec,
                             :quote_amount_exec, :created_at)

    net = Hash.new(0.to_d)
    opened = {}
    rows.each do |bot_id, side, status, price, amount, exec, quote_exec, created_at|
      exec, = Transaction.confirmed_exec_amounts(status, price, amount, exec, quote_exec)
      next if exec.blank? || exec.to_d.zero?

      net[bot_id] += side == 'sell' ? -exec.to_d : exec.to_d
      opened[bot_id] = [opened[bot_id], created_at].compact.min
    end

    # A clean zero only means "flat" while every row is in the same units. A bot whose own history
    # reaches back past an earlier restatement is not: one share bought before a 2-for-1 and one of
    # the resulting two sold after it also sums to zero, with a share still held. A bot that opened
    # AFTER that restatement has consistent units, and its zero is a real zero.
    restated_at = last_split_before(user, exchange, symbol, at)
    flat = net.select do |bot_id, amount|
      amount.zero? && (restated_at.nil? || opened[bot_id] >= restated_at)
    end
    Bot.where(id: net.keys - flat.keys)
  end
  private_class_method :bots_holding

  # When this symbol was last restated on this venue before now, or nil. Matched on the marker OR
  # on Alpaca's own activity type, so it reads correctly while the backfill is still walking rows
  # that have yet to be marked.
  def self.last_split_before(user, exchange, symbol, at)
    AccountTransaction.where(user: user, exchange: exchange, entry_type: :adjustment,
                             base_currency: symbol)
                      .where(transacted_at: ...at)
                      .filter_map do |row|
      raw = row.raw_data
      next unless raw.is_a?(Hash) &&
                  (raw['corporate_action'] == 'split' ||
                   Exchanges::Alpaca::SPLIT_TYPES.include?(raw['activity_type']))

      row.transacted_at
    end.max
  end
  private_class_method :last_split_before

  private

  # A split changes a share count with nothing bought or sold, so a bot's feed would otherwise show
  # its holding jump for no stated reason. One info line, dated at the split so it lands beside the
  # trades around it rather than on the day the sync happened to read it.
  #
  # Keyed on the corporate-action marker, not on `adjustment`: that entry type is generic — a
  # future venue correction lands in it, and an imported row carries no provenance at all — and a
  # line claiming a split that was not one is worse than no line.
  def log_split(at)
    return unless at.adjustment? && at.raw_data.is_a?(Hash) && at.raw_data['corporate_action'] == 'split'

    # The ledger first, and unconditionally: `announce_split` returns early for a split older than
    # the feed keeps, and whether a bot should be TOLD about a restatement is a different question
    # from whether its numbers changed.
    AccountTransactionSync.expire_restated_bots(
      user: @api_key.user, exchange: @exchange, symbol: at.base_currency
    )
    # An action dated ahead of today is imported now and takes effect later; the walk rightly
    # declines to apply it until then, so the bump has to happen again at that moment or the
    # pre-split position stays cached. No later sync will do it — by then the row is a duplicate.
    if at.transacted_at&.>(Time.current)
      Bot::ExpireRestatedMetricsJob.set(wait_until: at.transacted_at)
                                   .perform_later(@api_key.user, @exchange, at.base_currency)
    end
    AccountTransactionSync.announce_split(
      user: @api_key.user, exchange: @exchange, symbol: at.base_currency,
      at: at.transacted_at, ratio: at.raw_data['split_ratio']
    )
  rescue StandardError => e
    # The row is already stored, and a later sync will read it as a duplicate and never come back
    # here — so this must not abort the batch, and it must be loud. A bot left on a stale cache key
    # is a position read at the wrong basis, which is what the fleet's log scan is for.
    Rails.logger.error(
      "[#{@exchange.name_id}] Split side effects failed for #{at.base_currency} " \
      "tx_id=#{at.tx_id.inspect}: #{e.class}: #{e.message}"
    )
  end

  def duplicate?(entry, tx_id)
    if tx_id.nil?
      # (exchange, type, currency, amount, instant) — and deliberately NOT the key writing it.
      #
      # This used to be scoped to `api_key` on the reasoning that two keys on one venue meant two
      # sub-accounts. That stopped being true: a venue's history accumulates under whichever key was
      # current at the time, and a key can be replaced (its rows are nullified — see
      # `has_many :account_transactions, dependent: :nullify`), rotated, or superseded by a reading
      # key. Key-scoped, every one of those makes a re-read of the same history land a SECOND time.
      # The ledger then holds twice the coins the venue reports, and every P/L on the page goes
      # silent, because a balance and a ledger that disagree can state nothing.
      #
      # The sub-account this used to guard is not reachable anyway: `ApiKey` is unique per
      # (user, exchange, key_type), so a user cannot register two trading keys for two accounts on
      # one venue. If that ever changes, telling them apart needs a sub-account identity the ledger
      # does not record — the key is not one, because the same account changes keys.
      # WITHIN A SECOND, not the same instant. An exchange API timestamps to the millisecond and its
      # own CSV export writes whole seconds, rounded — `06:22:53.911` is `06:22:54` in the file — so
      # compared exactly, or bucketed by second, every Convert in the overlap between a file and a
      # sync lands a second time. Less than a full second apart is one event, from either side; a
      # row a full second later stays a row of its own.
      return same_event?(entry, scope) || replaced_trade?(entry)
    end

    # Only the STORED side is expanded to merged legs. A standalone leg arriving after its merged
    # adjustment is a duplicate and is skipped. The reverse is deliberately not done: skipping a
    # merged row because one of its legs is already stored would leave the ledger permanently
    # one-legged — a wrong share count that looks like data — where importing it merely double-counts
    # visibly and can be corrected.
    return true if stored_merged_activity_ids.include?(tx_id)
    return true if scope.exists?(tx_id: tx_id)

    # The other way round: a file imported BEFORE the first sync stored this row with no id, and
    # the venue's copy now arrives with one. An id that matches nothing is not yet a new row.
    same_event?(entry, scope.where(tx_id: nil))
  end

  # A row with no time is not the same event as anything; it is caught and logged on save.
  def same_event?(entry, rows)
    return false if entry[:transacted_at].blank?

    rows.where(entry_type: entry[:entry_type], base_currency: entry[:base_currency], base_amount: entry[:base_amount])
        .exists?(within_a_second(entry))
  end

  def within_a_second(entry)
    at = entry[:transacted_at]
    ['transacted_at > ? AND transacted_at < ?', at - 1.second, at + 1.second]
  end

  # A file Convert out of cash used to be read as a purchase, or a sale, and is now the swap pair
  # the venue books. A file imported under the old reading holds the purchase, and importing it
  # again must not add the pair on top. The PAIR is matched, as a whole, against one stored trade
  # within a second of it — the incoming leg as that trade's base and the outgoing leg as its quote
  # (a purchase), or the other way round (a sale) — so both legs are skipped or neither is; one leg
  # alone can coincide with an unrelated trade, and skipping it alone would leave a one-legged swap.
  def replaced_trade?(entry)
    return false unless entry[:group_id].to_s.start_with?('swapcsv_') && entry[:transacted_at].present?

    legs = @file_pairs.fetch(entry[:group_id], [])
    inn = legs.find { |leg| leg[:entry_type].to_s == 'swap_in' }
    out = legs.find { |leg| leg[:entry_type].to_s == 'swap_out' }
    return false unless inn && out && legs.size == 2

    legacy = scope.where(tx_id: nil).where(within_a_second(entry))
    legacy.exists?(entry_type: :buy, base_currency: inn[:base_currency], base_amount: inn[:base_amount],
                   quote_currency: out[:base_currency], quote_amount: out[:base_amount]) ||
      legacy.exists?(entry_type: :sell, base_currency: out[:base_currency], base_amount: out[:base_amount],
                     quote_currency: inn[:base_currency], quote_amount: inn[:base_amount])
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

  # A bot order and a ledger row are the same event under two ids, and which id the row carries is
  # the venue's choice. Binance names the trade after the order; Alpaca books one FILL activity per
  # partial fill, ids it after the fill, and names the order beside it — so matching on `tx_id`
  # alone left every Alpaca row unlinked, and the bot column empty for the whole history.
  #
  # `tx_id` first: it is the row's own identity, and a venue that uses the order id there means it.
  # Several fills of one order all point at that order, which is what the reader wants to see.
  def match_bot_transaction!(at)
    ids = [at.tx_id, (at.raw_data['order_id'] if at.raw_data.is_a?(Hash))].compact_blank
    return if ids.empty?

    candidates = Transaction.where(external_id: ids, exchange: @exchange).index_by(&:external_id)
    bot_tx = ids.filter_map { |id| candidates[id] }.first
    at.bot_transaction = bot_tx if bot_tx
  end
end
