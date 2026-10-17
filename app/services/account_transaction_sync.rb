class AccountTransactionSync
  def initialize(api_key)
    @api_key = api_key
    @exchange = api_key.exchange
  end

  def sync!(&progress)
    started = Time.current
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
    #
    # A stored watermark ahead of now (left behind before the cap below existed) is read as now.
    floor = @exchange.ledger_window&.ago
    start_time = @api_key.last_synced_at && [[@api_key.last_synced_at, started].min - 25.hours, floor].compact.max
    result = @exchange.get_ledger(api_key: @api_key, start_time: start_time)
    return result if result.failure?

    entries = result.data
    outcome = store!(entries, &progress)
    resolve_recent_assets

    # The watermark must come from the data — Time.current silently drops anything the fetch did not
    # return. It must also never advance past a row that failed to save: that row would fall outside
    # every future window, turning one malformed entry into a permanent hole. Nor may it pass the moment
    # this sync began: a row dated ahead (a split announced before its date) would carry it into the
    # future, and every fill, dividend and deposit before that date would fall outside every later window.
    max_seen = entries.filter_map { |entry| entry[:transacted_at] }.max
    watermark = [max_seen, outcome[:min_skipped]].compact.min || @api_key.last_synced_at
    @api_key.update!(last_synced_at: watermark && [watermark, started].min, last_sync_error: nil)
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
    refuse_unreadable_amounts!(entries)
    rekey_legacy_ids!(entries)
    total = entries.size
    imported = 0
    duplicates = 0
    skipped = 0
    last_percent = 0
    min_skipped = nil

    @file_pairs = entries.select { |entry| entry[:group_id].to_s.start_with?('swapcsv_') }.group_by { |entry| entry[:group_id] }
    @batch = entries
    @stored_ids = Set.new
    repeats = Hash.new(0)
    asset_ids = ledger_asset_ids(entries)

    entries.each_with_index do |entry, index|
      # A blank id identifies nothing, and several adapters produce one (Bybit's txID is empty for
      # internal transfers; Gemini/Hyperliquid/BingX build the id with .to_s on a field that can be
      # missing). Read it as nil so it dedups on the fallback identity instead of collapsing every
      # such row onto a single '' key — the partial unique index treats '' as a value.
      tx_id = entry[:tx_id].presence
      # A row its id already names is not one of the identical rows `same_event?` counts.
      @repeat = known_id?(tx_id) ? 0 : (repeats[event_key(entry)] += 1) - 1
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
        raw_data: entry[:raw_data] || {},
        base_asset_id: asset_ids[index]
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

      @stored_ids << at.id
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

    Bot.where(id: rows_naming(user, exchange, symbol).select(:bot_id))
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
    rows = rows_naming(user, exchange, symbol)
           .where(created_at: ...at)
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

  # The user's submitted orders on this venue for the asset a symbol names: recorded under that symbol, or
  # recorded with an asset the venue lists under it (spelling or symbol). A superset — a renamed asset's
  # older rows included — since the walk decides what a restatement actually moves.
  #
  # Guarded on the column: a migration that ran before transactions gained it calls this too.
  def self.rows_naming(user, exchange, symbol)
    rows = Transaction.submitted.where(exchange: exchange, bot_id: Bot.where(user: user).select(:id))
    return rows.where(base: symbol) unless Transaction.column_names.include?('base_asset_id')

    rows.where(base: symbol).or(rows.where(base_asset_id: Ticker.asset_ids_named(exchange.id, symbol)))
  end
  private_class_method :rows_naming

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

  # How long after a row is stored its asset may still be looked up: long enough for a first sync that
  # ran before the venue's listings arrived to be caught by the next daily one, and no longer — past
  # it, a name the venue lists now is not evidence of what it meant when the row was read.
  ASSET_CATCH_UP = 7.days

  # Which asset each entry moved, by the venue's rule. A failure here costs the drawing, never the
  # rows: they are stored with no asset and read by their symbol, as before.
  def ledger_asset_ids(entries)
    @exchange.ledger_asset_ids(entries)
  rescue StandardError => e
    Rails.logger.error("[#{@exchange.name_id}] Could not resolve ledger assets: #{e.class}: #{e.message}")
    []
  end

  def resolve_recent_assets
    recent = AccountTransaction.where(user: @api_key.user, exchange: @exchange, created_at: ASSET_CATCH_UP.ago..)
    AccountTransaction.resolve_base_assets!(recent, exchange: @exchange)
  rescue StandardError => e
    Rails.logger.error("[#{@exchange.name_id}] Could not resolve ledger assets: #{e.class}: #{e.message}")
  end

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

  # A NaN amount binds as NULL and raises at its insert, after the rows before it were saved; an
  # infinite one is no quantity at all. Either fails the whole batch before anything is written, so
  # a run never stores half a window — the next one fails the same way until the venue corrects it.
  def refuse_unreadable_amounts!(entries)
    entries.each do |entry|
      next if entry.values_at(:base_amount, :quote_amount, :fee_amount).all? { |n| !n.is_a?(Numeric) || n.finite? }

      raise ArgumentError, "Unreadable #{@exchange.name_id} ledger amount: tx_id=#{entry[:tx_id].inspect}"
    end
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
      return file_dust_twin?(entry) if file_dust?(entry)
      return file_transfer_twin?(entry) if file_transfer?(entry)

      return same_event?(entry, scope) || replaced_trade?(entry) || legacy_token_swap?(entry)
    end

    # Only the STORED side is expanded to merged legs. A standalone leg arriving after its merged
    # adjustment is a duplicate and is skipped. The reverse is deliberately not done: skipping a
    # merged row because one of its legs is already stored would leave the ledger permanently
    # one-legged — a wrong share count that looks like data — where importing it merely double-counts
    # visibly and can be corrected.
    return true if stored_merged_activity_ids.include?(tx_id)
    return true if scope.exists?(tx_id: tx_id)

    return api_dust_twin?(entry, tx_id) if api_dust?(entry, tx_id)
    return api_transfer_twin?(entry, tx_id) if api_transfer?(entry, tx_id)

    # The other way round: a file imported BEFORE the first sync stored this row with no id, and
    # the venue's copy now arrives with one. An id that matches nothing is not yet a new row.
    same_event?(entry, scope.where(tx_id: nil))
  end

  # A row with no time is not the same event as anything; it is caught and logged on save.
  #
  # A row this run stored is no evidence: an export repeats identical rows in one second, each a real
  # credit, and matched against each other all but the first were dropped. Rows stored BEFORE the run
  # count one for one — the Nth identical row in the batch is a duplicate while N such rows were already
  # there — so the same file imported twice still adds nothing.
  #
  # A pairing between the export and the API passes the rows it decides itself as `governed`: those are
  # not evidence either.
  def same_event?(entry, rows, &governed)
    return false if entry[:transacted_at].blank?

    same_events(entry, rows).count { |row| !@stored_ids.include?(row.id) && !governed&.call(row) } > @repeat
  end

  def event_key(entry)
    [entry[:entry_type].to_s, entry[:base_currency], entry[:base_amount].to_d, entry[:transacted_at]]
  end

  def known_id?(tx_id)
    tx_id.present? && (stored_merged_activity_ids.include?(tx_id) || scope.exists?(tx_id: tx_id))
  end

  # A file imported before "Token Swap - Distribution" was read as an airdrop stored it as other income.
  LEGACY_TOKEN_SWAP = 'Token Swap - Distribution'.freeze

  def legacy_token_swap?(entry)
    return false unless entry[:entry_type].to_s == 'airdrop' && entry.dig(:raw_data, 'operation') == LEGACY_TOKEN_SWAP

    same_event?(entry.merge(entry_type: :other_income), scope.where("json_extract(raw_data, '$.operation') = ?", LEGACY_TOKEN_SWAP))
  end

  def same_events(entry, rows)
    rows.where(entry_type: entry[:entry_type], base_currency: entry[:base_currency], base_amount: entry[:base_amount])
        .where(within_a_second(entry))
  end

  # ---- one transfer, as Binance's export and its API each describe it --------------------------------
  #
  # The export dates a deposit when it was CREDITED, seconds to minutes after the API's `insertTime`,
  # and writes a withdrawal GROSS — the amount plus the network fee — at the moment it completed,
  # where the API row is net, with the fee beside it, at the moment it was requested. Neither copy is
  # within a second of the other, so the rule above would store the transfer twice, whichever came
  # first. These two shapes are paired on their own terms instead, and only with each other:
  #
  # - a deposit, same coin and amount, credited in the file no earlier than the API saw it and at most
  #   CREDIT_LAG after;
  # - a withdrawal whose `amount` + `transactionFee` is the file's amount, the file dated within a
  #   second of the API's `completeTime`.
  #
  # Which row is which is read off the data, not off a missing id (several adapters store API rows
  # without one): the file row is the importer's `{"operation" => "Deposit" | "Withdraw"}`, the API
  # row has an id and the capital endpoint's `insertTime` / `completeTime`. A row of neither shape is
  # matched as before.
  #
  # One file row pairs with ONE API row, and the pairing is written down, so it holds across calls (a
  # sync stores its history in chunks; a file is imported on its own): an API row arriving first-time
  # takes the file row's empty id, and a file row arriving after the API row stamps that row's raw
  # with `file_twin_at`. Whichever came first is the row that stays, fields and all. Two candidates
  # either way are left as a visible double — dropping a real second deposit would be silent.
  FILE_TRANSFERS = { 'deposit' => 'Deposit', 'withdrawal' => 'Withdraw' }.freeze
  API_TRANSFER_TIMES = { 'deposit' => 'insertTime', 'withdrawal' => 'completeTime' }.freeze
  CREDIT_LAG = 10.minutes

  def file_transfer?(row)
    operation = FILE_TRANSFERS[row[:entry_type].to_s]
    operation.present? && (row[:raw_data] || {})['operation'] == operation
  end

  def api_transfer?(row, tx_id = row[:tx_id])
    time = API_TRANSFER_TIMES[row[:entry_type].to_s]
    time.present? && tx_id.present? && (row[:raw_data] || {}).key?(time)
  end

  # A present but unreadable `completeTime` pairs with nothing.
  def completed_at(raw)
    ActiveSupport::TimeZone['UTC'].strptime(raw['completeTime'].to_s, '%Y-%m-%d %H:%M:%S')
  rescue ArgumentError
    nil
  end

  # What the file says for this API row: its amount, and the window its date falls in.
  def file_side(api)
    raw = api[:raw_data]
    if api[:entry_type].to_s == 'deposit'
      at = api[:transacted_at]
      [raw['amount'].to_d, at, at + CREDIT_LAG, :closed]
    else
      complete = completed_at(raw)
      complete && [raw['amount'].to_d + raw['transactionFee'].to_d, complete - 1.second, complete + 1.second, :open]
    end
  end

  def pairs?(api, file)
    return false unless file[:entry_type].to_s == api[:entry_type].to_s && file[:base_currency] == api[:base_currency]

    amount, from, to, bounds = file_side(api)
    return false unless amount && file[:transacted_at]

    at = file[:transacted_at]
    file[:base_amount].to_d == amount && (bounds == :open ? at > from && at < to : at >= from && at <= to)
  end

  def api_transfer_twin?(entry, tx_id)
    # The second-wide rule still holds against every id-less row except a file transfer: that pairing
    # is decided below.
    return true if same_event?(entry, scope.where(tx_id: nil)) { |row| file_transfer?(row) }

    amount, from, to, bounds = file_side(entry)
    return false unless amount

    # Raced: someone took the row between the read and the write. Read once more, then store.
    2.times do
      rows = scope.where(tx_id: nil, entry_type: entry[:entry_type], base_currency: entry[:base_currency], base_amount: amount)
                  .where(bounds == :open ? 'transacted_at > ? AND transacted_at < ?' : 'transacted_at >= ? AND transacted_at <= ?', from, to)
                  .select { |row| file_transfer?(row) }
      return false unless rows.one?
      return true if take!(rows.first, tx_id)
    end
    false
  rescue ActiveRecord::RecordNotUnique
    true # the id was stored meanwhile
  end

  def take!(row, tx_id)
    AccountTransaction.where(id: row.id, tx_id: nil).update_all(tx_id: tx_id) == 1
  end

  def file_transfer_twin?(entry)
    at = entry[:transacted_at]
    return false if at.blank?
    # The same file row stored already — claimed or not — or any row this pairing does not govern.
    return true if same_event?(entry, scope) { |row| api_transfer?(row) }

    2.times do
      candidates = api_rows_for(entry)
      return true if twinned?(candidates, at)

      free = candidates.reject { |row| row.raw_data.key?('file_twin_at') }
      return false unless free.one?

      # The file's other rows that could take this API row too; one already paired is not a rival.
      rivals = @batch.count do |other|
        other[:tx_id].blank? && file_transfer?(other) && pairs?(free.first, other) &&
          (other.equal?(entry) || !paired?(other))
      end
      return false unless rivals == 1
      return true if stamp!(free.first, at)
    end
    false
  end

  # A file row whose pairing is written down: stored with an API row's id, or its date stamped on one.
  # Stored without either, it is still free.
  def paired?(entry)
    same_events(entry, scope.where.not(tx_id: nil)).any? { |row| !api_transfer?(row) } ||
      twinned?(api_rows_for(entry), entry[:transacted_at])
  end

  def twinned?(rows, at)
    rows.any? { |row| (twin = file_twin_at(row)) && (twin - at).abs < 1 }
  end

  # The stored API rows this file row pairs with. An API row is dated when it was requested, so a
  # withdrawal's is at or before its completion.
  def api_rows_for(entry)
    at = entry[:transacted_at]
    rows = scope.where.not(tx_id: nil).where(entry_type: entry[:entry_type], base_currency: entry[:base_currency])
    rows = if entry[:entry_type].to_s == 'deposit'
             rows.where('transacted_at >= ? AND transacted_at <= ?', at - CREDIT_LAG, at)
           else
             rows.where('transacted_at < ?', at + 1.second)
           end
    rows.select { |row| api_transfer?(row) && pairs?(row, entry) }
  end

  def file_twin_at(row)
    Time.iso8601(row.raw_data['file_twin_at'].to_s)
  rescue ArgumentError
    nil
  end

  def stamp!(row, at)
    AccountTransaction.where(id: row.id).where("json_extract(raw_data, '$.file_twin_at') IS NULL")
                      .update_all(["raw_data = json_set(raw_data, '$.file_twin_at', ?)", at.utc.iso8601(6)]) == 1
  end

  # ---- one dust conversion, as Binance's export and its API each describe it --------------------------
  #
  # The API books a sweep per coin: the coin out, and the BNB in GROSS (`transferedAmount`) with the 2%
  # charge beside it (`serviceChargeAmount`). The export writes the BNB that arrived, NET of the charge,
  # and dates the sweep up to a second after `operateTime`. Matched row by row, the coin legs met and
  # the BNB legs never did — half a conversion paired, the BNB counted twice — and a second off, both
  # legs were stored twice.
  #
  # A conversion is paired whole instead, and only with the other source: the API's coin and amount out
  # and its gross less the charge in, against the one export group (`dustcsv_…`) holding that out leg
  # and that in leg, each once, dated within DUST_REACH of `operateTime` either side. The export rounds the
  # net on its own, so its BNB may sit DUST_ROUNDING (one unit of Binance's 8th decimal) off. One leg alone pairs
  # with nothing. Written down as a transfer is: an API conversion arriving first-time gives both file
  # legs its ids, and a file group arriving after the API stamps the API rows with `file_twin_at`. Two
  # candidates either way are a visible double.
  DUST_LEGS = %w[swap_out swap_in].freeze
  DUST_REACH = 1.second
  DUST_ROUNDING = BigDecimal('0.00000001')

  def file_dust?(row)
    row[:group_id].to_s.start_with?('dustcsv') && DUST_LEGS.include?(row[:entry_type].to_s)
  end

  def api_dust?(row, tx_id = row[:tx_id])
    tx_id.to_s.start_with?('dust-') && DUST_LEGS.include?(row[:entry_type].to_s) && (row[:raw_data] || {}).key?('transferedAmount')
  end

  # What an API dust row is a leg of: the coin and amount out, and the BNB in, net of the charge.
  def conversion(raw)
    [raw['fromAsset'], raw['amount'].to_d, raw['transferedAmount'].to_d - raw['serviceChargeAmount'].to_d]
  end

  def dust_leg(row, (coin, amount, net))
    case row[:entry_type].to_s
    when 'swap_out' then :out if row[:base_currency] == coin && row[:base_amount].to_d == amount
    when 'swap_in' then :in if row[:base_currency] == 'BNB' && (row[:base_amount].to_d - net).abs <= DUST_ROUNDING
    end
  end

  def dust_reach(at)
    ['transacted_at >= ? AND transacted_at <= ?', at - DUST_REACH, at + DUST_REACH]
  end

  # The one file group among `legs` (rows that match the conversion) holding both of its legs, each
  # once: [out, in], or nil.
  def file_dust_legs(legs)
    groups = legs.group_by { |row| row[:group_id] }.values.map { |rows| rows.group_by { |row| row[:entry_type].to_s } }
                 .select { |rows| rows.key?('swap_out') && rows.key?('swap_in') }
    return unless groups.one? && groups.first.values.all?(&:one?)

    [groups.first['swap_out'].first, groups.first['swap_in'].first]
  end

  def api_dust_twin?(entry, tx_id)
    at = entry[:transacted_at]
    return false if at.blank?
    return true if same_event?(entry, scope.where(tx_id: nil)) { |row| file_dust?(row) }

    id = tx_id.delete_suffix('-out').delete_suffix('-in')
    coin, amount, net = conversion(entry[:raw_data])
    2.times do
      # The database compares the amounts, as the rule above does: a stored amount is what the column kept.
      near = scope.where(tx_id: nil).where('group_id LIKE ?', 'dustcsv%').where(dust_reach(at))
      out, inn = file_dust_legs(near.where(entry_type: :swap_out, base_currency: coin, base_amount: amount).to_a +
                                near.where(entry_type: :swap_in, base_currency: 'BNB',
                                           base_amount: (net - DUST_ROUNDING)..(net + DUST_ROUNDING)).to_a)
      return false unless out
      return true if both? { take!(out, "#{id}-out") && take!(inn, "#{id}-in") }
    end
    false
  rescue ActiveRecord::RecordNotUnique
    true # an id stored meanwhile
  end

  def file_dust_twin?(entry)
    at = entry[:transacted_at]
    return false if at.blank?
    # The same file row stored already — claimed or not — or any row this pairing does not govern.
    return true if same_event?(entry, scope) { |row| api_dust?(row) }

    2.times do
      # Only a whole API conversion: one stored a leg short (a sync cut off between them) leaves the file's
      # BNB credit standing, a visible double rather than a lost one.
      conversions = scope.where('tx_id LIKE ?', 'dust-%').where(entry_type: DUST_LEGS).where(dust_reach(at))
                         .select { |row| api_dust?(row) && dust_leg(entry, conversion(row.raw_data)) }
                         .group_by(&:group_id).values.select { |rows| rows.map(&:entry_type).sort == DUST_LEGS.sort }
      return true if conversions.any? { |rows| twinned?(rows, at) }

      free = conversions.reject { |rows| rows.any? { |row| row.raw_data.key?('file_twin_at') } }
      return false unless free.one?

      api = free.first
      reach = api.first.transacted_at
      file = @batch.select do |other|
        other[:tx_id].blank? && file_dust?(other) && other[:transacted_at] && (other[:transacted_at] - reach).abs <= DUST_REACH
      end
      out, = file_dust_legs(file.select { |other| dust_leg(other, conversion(api.first.raw_data)) })
      return false unless out && out[:group_id] == entry[:group_id]
      return true if both? { api.all? { |row| stamp!(row, at) } }
    end
    false
  end

  # Both legs or neither.
  def both?
    AccountTransaction.transaction(requires_new: true) { yield || raise(ActiveRecord::Rollback) } || false
  end

  # Exactly a second apart is one event only across sources: Binance's API cuts a Convert to the
  # whole second (14:29:18) where its export rounds it up (14:29:19). Within one source the clocks
  # agree, and identical rows a second apart are two events.
  def within_a_second(entry)
    at = entry[:transacted_at]
    other = entry[:tx_id].present? ? 'tx_id IS NULL' : 'tx_id IS NOT NULL'
    ["(transacted_at > ? AND transacted_at < ?) OR (#{other} AND transacted_at >= ? AND transacted_at <= ?)",
     at - 1.second, at + 1.second, at - 1.second, at + 1.second]
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

  # An adapter that changed how it ids a row names the old id as :legacy_tx_id (Coinbase stored an
  # adjusted fill under the trade_id it shares with its FILL; it is now ided by its entry_id). The
  # row stored under the old id takes the new one before dedup, so it is neither stored twice nor
  # left hiding the row that really owns the old id. The stored raw entry_id decides; a row without
  # one is matched on the same event.
  def rekey_legacy_ids!(entries)
    entries.each do |entry|
      legacy = entry[:legacy_tx_id].presence or next
      tx_id = entry[:tx_id].presence or next
      next if scope.exists?(tx_id:)

      rows = scope.where(tx_id: legacy)
      entry_id = entry.dig(:raw_data, 'entry_id')
      row = (rows.find_by("json_extract(raw_data, '$.entry_id') = ?", entry_id) if entry_id) ||
            same_events(entry, rows.where("json_extract(raw_data, '$.entry_id') IS NULL")).first
      row&.update_columns(tx_id:)
    end
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
