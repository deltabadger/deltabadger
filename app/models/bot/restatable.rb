# frozen_string_literal: true

# What a bot knows about its own share counts being restated.
#
# A bot's `transactions` table records orders and nothing else, so a corporate action is invisible
# to it: after a 10-for-1 split it goes on believing it owns the number of shares it bought. The
# only account of the event is the broker's, and it is already stored — the account sync books it
# as an `adjustment` ledger row marked `corporate_action`, the same rows `Tracker::Ledger` and
# `Tax::Methods::Fifo` already restate against.
#
# This turns those rows into the events the metrics walks fold in.
module Bot::Restatable
  extend ActiveSupport::Concern

  # How long a live price stays out of trust after a restatement.
  #
  # A venue's "latest trade" is the last trade that actually happened, and a corporate action is
  # effective before the market opens — so between booking one and the first trade on the new
  # basis, the live mark is still the OLD price. Multiplied by a count this code has just
  # restated, that is the factor's worth of imaginary money, and a rebalancer reading it sees an
  # asset wildly overweight and sells real shares to correct a move that never happened.
  #
  # ponytail: a blunt window, because the shared `get_tickers_prices` returns prices and no
  # timestamps. Alpaca's snapshot carries `latestTrade.t` right beside the price it does return;
  # plumb that through and this becomes the exact question — is the mark newer than the action —
  # instead of a wait. Two days so a session always intervenes, weekend or holiday.
  SPLIT_PRICE_QUARANTINE = 2.days

  # `[[at, key, factor], ...]`, oldest first: what the holding under `key` is multiplied by, and when.
  # `holdings` is `{ key => [asset_id or nil, [strings its rows were recorded under]] }`
  # (see #split_holdings).
  #
  # ponytail: read per walk, and each walk memoizes it. A bot with enough corporate actions for
  # that lookup to show up belongs in the metrics cache with the rest of them.
  def split_events(holdings = split_holdings)
    grouped_split_rows(holdings).filter_map { |(key, date), group| split_event(key, date, group) }
                                .select { |at, _key, _factor| at <= Time.current }
                                .sort_by { |at, key, _factor| [at, key] }
  end

  # The holdings a split can apply to. A single-pair bot sums every row into one position, whatever symbol
  # a row was recorded under, so it has one holding: its ticker's. A composition bot has one per asset
  # (Bot::Composition::Measurable overrides this).
  def split_holdings(_payload = nil)
    return {} unless transactions.submitted.exists?

    strings = transactions.submitted.distinct.pluck(:base).compact_blank
    key = try(:ticker)&.base || strings.first
    { key => [try(:base_asset_id), strings] }
  end

  # Whether a live price can be multiplied by this bot's holdings at all.
  #
  # Two ways it cannot. A restatement newer than the market's last chance to reprice the security:
  # the count has moved and the venue's last trade has not, so the walk's own last-known prices —
  # restated alongside the position — are the honest reading and the live ones are not. Or a split
  # this bot can see but cannot size, where the holdings can NEVER be brought onto the same basis
  # as the price; that one does not expire.
  #
  # `metrics_data` carries `:restated_at`, the last restatement that actually moved a position. A
  # split before the bot's first fill, or after it sold out, moves nothing and is no reason to
  # stand a bot down.
  #
  # ponytail: the unresolved case is deliberately not narrowed to the symbols still held. It should
  # not happen at all, and while it does, over-broad means safe.
  def restated_prices_untrusted?(metrics_data = nil)
    return true if unresolved_split?(split_holdings(metrics_data.is_a?(Hash) ? metrics_data : nil))

    restated_at = metrics_data.is_a?(Hash) ? metrics_data[:restated_at] : nil
    restated_at.present? && restated_at > SPLIT_PRICE_QUARANTINE.ago
  end

  # A split this bot can see and cannot size — one leg with no counterpart, or two sources naming
  # different factors. Its position stays on the old basis while the market moves to the new one,
  # which no amount of waiting fixes.
  def unresolved_split?(holdings = split_holdings)
    effective_split_rows(holdings).any? { |(_key, date), group| date.nil? || resolved_factor(group).nil? }
  end

  # Everything a bot has cached is computed from a position a corporate action moves, so when one
  # lands they all go. Order sizing forces its own recompute and would be right regardless, but
  # `Bot::Rebalancer#rebalance_targets` reads `metrics_with_current_prices` UNFORCED — a stale hash
  # there is a sell sized against a tenth of a position.
  #
  # The generation moves FIRST, and it is what actually makes this safe. Deleting alone loses a
  # race: a walk that read the ledger just before the split was stored can finish just after and
  # write its pre-split answer back into the hole, where nothing downstream can tell it apart from
  # a fresh one. Past the bump that writer is computing an old key and can only write there. The
  # deletes then just reclaim the entries nothing will read again.
  def expire_restated_metrics!
    stale_keys = restated_metrics_cache_keys
    increment!(:restatement_generation)
    stale_keys.each { |key| Rails.cache.delete(key) }
  end

  def restated_metrics_cache_keys
    %i[metrics_cache_key metrics_with_current_prices_cache_key
       metrics_with_current_prices_and_candles_cache_key]
      .filter_map { |name| send(name) if respond_to?(name, true) }
  end

  private

  # The actions already in effect. A broker can store a pending one dated ahead of today, and a
  # position restated before its split is simply wrong — as is standing a bot's automation down
  # for an event that has not happened.
  def effective_split_rows(holdings)
    grouped_split_rows(holdings).reject do |_key, group|
      group.map(&:transacted_at).compact.min.try(:>, Time.current)
    end
  end

  # The marked split rows this bot is eligible for, grouped per holding and effective date.
  #
  # A corporate action is an event of the SECURITY on an effective date, so that is the key: two
  # venues reporting one restatement must apply it once, and they need not agree on the hour — every
  # report that applies to one holding on one date is reconciled together.
  #
  # A report applies to a holding when:
  # - it names exactly one asset on its venue (by spelling or symbol): only to that asset's holding;
  # - it names none or several: to every holding with a row recorded under its string;
  # - the holding has no asset (rows recorded before orders stored theirs): whenever it names the string.
  def grouped_split_rows(holdings)
    string_pairs, asset_pairs = traded_pairs
    return {} if string_pairs.empty? && asset_pairs.empty?

    exchange_ids = (string_pairs + asset_pairs).map(&:first).uniq
    rows = AccountTransaction.where(user_id: user_id, entry_type: :adjustment, exchange_id: exchange_ids)
                             .where(base_currency: string_pairs.map(&:last) + traded_asset_names(asset_pairs))
                             .select { |row| split_row?(row) }
    report_assets = Hash.new { |hash, (exchange_id, name)| hash[[exchange_id, name]] = split_report_asset(exchange_id, name) }

    rows.each_with_object({}) do |row, groups|
      report_asset = report_assets[[row.exchange_id, row.base_currency]]
      holdings.each do |key, (asset_id, strings)|
        applies = if asset_id && report_asset
                    report_asset == asset_id && asset_pairs.include?([row.exchange_id, asset_id])
                  else
                    Array(strings).include?(row.base_currency) && string_pairs.include?([row.exchange_id, row.base_currency])
                  end
        (groups[[key, effective_date(row)]] ||= []) << row if applies
      end
    end
  end

  # Every name the venues list the traded assets under — spelling and symbol — which a report about them
  # may use.
  def traded_asset_names(asset_pairs)
    return [] if asset_pairs.empty?

    Ticker.where(exchange_id: asset_pairs.map(&:first).uniq, base_asset_id: asset_pairs.map(&:last).uniq)
          .includes(:base_asset).flat_map { |ticker| [ticker.base_spelling, ticker.base_asset&.symbol] }
          .compact_blank.uniq
  end

  # The one asset a report's name stands for on its venue, by spelling or symbol; nil for none or several.
  def split_report_asset(exchange_id, name)
    spelled = Ticker.where(exchange_id:).where('tickers.base LIKE ?', "%#{Ticker.sanitize_sql_like(name)}")
                    .select { |ticker| ticker.base_spelling.casecmp?(name) }.map(&:base_asset_id)
    symbolled = Ticker.where(exchange_id:, base_asset_id: Asset.where('upper(symbol) = ?', name.upcase).select(:id))
                      .pluck(:base_asset_id)
    ids = (spelled + symbolled).uniq
    ids.first if ids.one?
  end

  def split_row?(row)
    row.raw_data.is_a?(Hash) && row.raw_data['corporate_action'] == 'split'
  end

  # The (venue, symbol) and (venue, asset) pairs this bot actually traded — its eligibility, and
  # deliberately not its current venue. A bot can be moved between brokers and its orders keep the one
  # they were placed on, so reading the present venue would both miss the restatement that moved its
  # position and apply one that never touched it. Every row's symbol counts, resolved or not, so what a
  # string made eligible before orders recorded their asset stays eligible.
  #
  # `submitted` rather than executed: an order that never filled bought nothing, so a pair it alone
  # contributes can only apply a factor to a position of zero.
  def traded_pairs
    rows = transactions.submitted.distinct.pluck(:exchange_id, :base, :base_asset_id)
    [rows.filter_map { |exchange_id, base, _id| [exchange_id, base] if exchange_id && base.present? }.uniq,
     rows.filter_map { |exchange_id, _base, id| [exchange_id, id] if exchange_id && id }.uniq]
  end

  def effective_date(row)
    row.transacted_at&.utc&.to_date
  end

  # One event, or none. Sources that disagree about the factor restate nothing: a share count is
  # not something to guess at, and a wrong factor is a wrong position in every order this bot sizes.
  #
  # Timed at the EARLIEST instant the group reports, not at the key's date boundary. The date is a
  # de-duplication key and nothing more: `transacted_at` is written by parsing the venue's date
  # string in `Time.zone`, so reconstructing an instant from a UTC date would move the event by a
  # day for every zone ahead of UTC — and `Time.zone` is per-request here. The row's own instant is
  # zone-independent and already says when the venue booked it.
  def split_event(key, date, group)
    return nil if date.nil?

    factor = resolved_factor(group)
    return nil unless factor

    [group.map(&:transacted_at).min, key, factor]
  end

  # One factor, or none. A row that names no factor is silent, not dissenting: Alpaca can ship the
  # add leg alone and the merged pair a sync later, and the sync keeps both on purpose — the first
  # says a split happened without saying by how much, the second says both. Treating that pair as a
  # conflict would leave the position un-restated forever, which is the failure this exists to end.
  # Two rows naming DIFFERENT factors resolve to nothing: a share count is not something to guess
  # at, and a wrong factor is a wrong position in every order this bot sizes.
  def resolved_factor(group)
    factors = group.filter_map { |row| split_factor(row) }.uniq
    factors.size == 1 ? factors.first : nil
  end

  # "10:1" -> 10. Fails closed on anything else — a blank, a single number, a zero or negative
  # side, words, or extra parts. Nothing is restated on a factor that could not be read.
  def split_factor(row)
    parts = row.raw_data['split_ratio'].to_s.split(':')
    return nil unless parts.size == 2 && parts.all? { |part| part.match?(/\A\d+(\.\d+)?\z/) }

    new_count, old_count = parts.map(&:to_d)
    return nil unless new_count.positive? && old_count.positive?

    new_count / old_count
  end
end
