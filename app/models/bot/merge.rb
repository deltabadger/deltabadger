# Several DCA bots into one basket that owns all of their history.
#
# The user picks the bots on the dashboard; the first pick is the anchor. Bots can merge when they
# spend the same currency and some exchange lists every asset they hold at that currency. The new bot
# lives on the anchor's exchange when it lists everything, else on a connected exchange that does,
# else on any that does — the modal and the bar say which, and the modal lets the user move it to any
# other venue that lists everything (`exchanges`).
# It takes the anchor's interval and amount, sums every source's current per-asset weights and
# normalises them, starts stopped with every rule off, and receives every order, activity row and
# membership the sources had. The sources are then soft-deleted, exactly as the Delete button leaves a
# bot: a `deleted` row with no history.
#
# Three uses, one object: the tile asks `mergeable?` and `venues_for` to describe itself, the modal
# asks `reason`, `exchange` and `bot` to say what will happen, and the POST calls `perform!`.
#
# The fence is each venue's own trading lock (Bot::VenueLease), taken on every source's exchange for the
# duration of the transaction: while it is held no tick can be dispatched or claimed on those venues,
# and if one is running the merge is refused instead of racing it.
class Bot::Merge
  TYPES = %w[Bots::DcaMultiAsset Bots::DcaIndex].freeze

  # Can this bot be picked at all? Server-computed per tile; re-run in perform! against fresh rows.
  # An index bot that never derived its composition has no rows and no history: nothing to merge.
  def self.mergeable?(bot)
    TYPES.include?(bot.type) && !bot.archived? && !bot.deleted? && !bot.executing? &&
      !bot.rebalance_pending? && !bot.liquidation_in_flight? && !bot.redeploy_in_flight? &&
      bot.bot_index_assets.in_index.exists?
  end

  # The chips a tile carries for the bar: current members, weight order.
  def self.members(bot)
    bot.bot_index_assets.in_index.includes(:asset).order(target_allocation: :desc).map(&:asset)
  end

  # { asset_id => [exchange_id, ...] }: where each asset trades at the quote, on venues that trade.
  # An asset listed nowhere is absent, not empty.
  def self.venues_for(asset_ids, quote_asset_id)
    Ticker.available.trading_enabled.joins(:exchange).merge(Exchange.tradeable)
          .where(base_asset_id: asset_ids, quote_asset_id:)
          .pluck(:base_asset_id, :exchange_id)
          .group_by(&:first).transform_values { |pairs| pairs.map(&:last).uniq.sort }
  end

  # The exchanges the user has a trading key on, in the order the keys were added.
  def self.connected_exchange_ids(user)
    user.api_keys.where(key_type: :trading).order(:id).pluck(:exchange_id).uniq
  end

  attr_reader :user, :bots, :error

  # exchange_id: the venue the user picked in the modal, if they did; nil leaves the choice to #exchange.
  def initialize(user, ids, exchange_id: nil)
    @user = user
    @ids = Array(ids).map(&:to_i).uniq # request order: the first id is the anchor
    @exchange_id = exchange_id
    load_bots
  end

  def anchor = bots.first

  # An i18n key under errors.bots.merge — with its interpolation when it names a bot or a venue — or
  # nil when the merge can go ahead. The selection the user confirmed is the selection: an id that is
  # gone — deleted elsewhere, a stranger's — refuses the whole merge rather than quietly promoting the
  # next bot to anchor.
  def reason
    return :missing if bots.size < @ids.size
    return :too_few if bots.size < 2

    unavailable = bots.find { |bot| !self.class.mergeable?(bot) }
    return [:unavailable, { label: unavailable.label }] if unavailable
    return :quote unless bots.all? { |bot| bot.quote_asset_id.to_i == anchor.quote_asset_id.to_i }
    return :nothing_to_buy if tradeable_weights.empty?
    return :no_common_exchange if exchange.nil?

    # A resting order is polled through its bot's exchange; on another venue it would never be found.
    open = bots.find { |bot| bot.exchange_id != exchange.id && bot.transactions.waiting.exists? }
    return [:open_orders, { label: open.label, exchange: open.exchange.name }] if open

    # A sale of units a source never bought would consume another source's units in the merged pool,
    # which the venue contradicts. The books are read fresh: a cached payload may predate the flag, and
    # verify_books! compares against them.
    @books = bots.index_with { |bot| bot.respond_to?(:metrics) ? bot.metrics(force: true) : {} }
    external = bots.find { |bot| @books[bot][:external_sales] }
    return [:external_sales, { label: external.label }] if external

    nil
  end

  # The one chronological walk over the combined rows IS the merged bot's books. Figures the pool
  # derives may differ from the sources' — a sale realised at the pooled cost, a buy that spends
  # another source's proceeds — and that is the merged history, not an error. What it must not do is
  # lose or invent money or units: a swap of A's in flight while B bought reads B's buy as spending
  # A's cash, and A's buy-back then spends money the books no longer hold. That merge is refused after
  # the fact, by measuring. Raised inside the transaction, so nothing of it stays.
  class Interleaved < StandardError; end

  # The reason as the user reads it, or nil.
  def refusal
    reason && self.class.translate(reason)
  end

  # Bot::Split shares it under its own scope.
  def self.translate(reason, scope: :merge)
    key, interpolation = Array(reason)
    I18n.t("errors.bots.#{scope}.#{key}", **(interpolation || {}))
  end

  # Where the merged bot lives: the venue the user picked, if it lists every member at the quote — a
  # pick that does not is refused, never swapped for another. Without a pick: the anchor's exchange if
  # it lists every member, else a connected exchange that does, else any that does; nil when none does.
  def exchange
    return @exchange if defined?(@exchange)

    ids = common_exchange_ids
    chosen = if @exchange_id then @exchange_id.presence_in(ids)
             elsif ids.include?(anchor.exchange_id) then anchor.exchange_id
             else (self.class.connected_exchange_ids(user) & ids).first || ids.min
             end
    @exchange = chosen && Exchange.find_by(id: chosen)
  end

  # The venues the user may pick from: every one that lists all the members at the quote, and — when a
  # source has orders resting — only the venue those orders are polled through (none when two sources
  # rest on different venues).
  def exchanges
    resting = bots.select { |bot| bot.transactions.waiting.exists? }.map(&:exchange_id).uniq
    ids = if resting.empty? then common_exchange_ids
          elsif resting.one? then common_exchange_ids & resting
          else []
          end
    Exchange.where(id: ids).order(:name).to_a
  end

  # The sources' exchanges the merged bot is not on: their assets stay where they were bought.
  def other_exchanges
    bots.map(&:exchange).uniq.reject { |venue| venue.id == exchange&.id }
  end

  # {asset_id_string => weight}: the sum of every source's current target weights over the members that
  # still trade somewhere at the quote, normalised by the basket's own normaliser (3 dp, sums to 1).
  # Anchor first, so the label's "first three" are the anchor's.
  def allocations
    @allocations ||= bot.normalize_allocations(tradeable_weights)
  end

  # The unsaved merged bot: what the modal previews and what perform! saves. Nothing but the schedule
  # in its settings, so every rule reads off. The exchange is assigned AFTER the settings: the
  # limit-orders decorator on `exchange=` writes into whatever settings hash is there, and a settings
  # hash assigned later would wipe the flag Hyperliquid requires.
  def bot
    @bot ||= begin
      merged = user.bots.new(type: 'Bots::DcaMultiAsset', status: :stopped, position: anchor.position,
                             settings: { 'quote_asset_id' => anchor.quote_asset_id,
                                         'quote_amount' => anchor.quote_amount,
                                         'interval' => anchor.interval,
                                         'weighting' => 'manual' })
      merged.exchange = exchange
      merged
    end
  end

  # The members the merged bot will buy, in weight order — for the modal and the label.
  def assets
    by_id = Asset.where(id: allocations.keys).index_by { |asset| asset.id.to_s }
    allocations.keys.filter_map { |id| by_id[id] }
  end

  # @return [Bots::DcaMultiAsset, nil] the merged bot, or nil with #error set
  def perform!
    return fail!(reason) if reason

    venues = bots.map(&:exchange).uniq
    merged = nil
    held = Bot::VenueLease.hold(venues, holder: "Bot::Merge for #{@ids.inspect}") do
      Bot.transaction do
        load_bots(lock: true)
        return fail!(reason) if reason
        # A source moved to another venue since the leases were taken trades under a lock we do not
        # hold: refuse rather than merge a bot whose tick could be running right now.
        return fail!(unavailable) unless bots.all? { |source| venues.any? { |venue| venue.id == source.exchange_id } }

        merged = write!
        verify_books!(merged)
      end
      # After the commit, still under the leases: the sources' ticks scheduled for later, and any the
      # dispatcher parked behind a lease. Solid Queue is another database, so a cancel before the
      # commit could not be rolled back with a failed write; nothing here can be claimed either way.
      bots.each { |source| cancel_queued_jobs(source) }
    rescue Interleaved
      return fail!(:interleaved)
    end
    return fail!(unavailable) unless held

    merged&.reload
  end

  private

  def load_bots(lock: false)
    scope = user.bots.not_deleted.where(id: @ids)
    scope = scope.lock if lock
    by_id = scope.index_by(&:id)
    @bots = @ids.filter_map { |id| by_id[id] }
    @bot = nil
    @allocations = nil
    @weights = nil
    @venues = nil
    remove_instance_variable(:@exchange) if defined?(@exchange)
  end

  # Every source's current target weights, summed per asset, anchor first.
  def weights
    @weights ||= bots.each_with_object(Hash.new(0.0)) do |source, summed|
      source.current_allocations.each { |member| summed[member[:asset].id] += member[:target_allocation].to_f }
    end
  end

  def venues
    @venues ||= self.class.venues_for(weights.keys, anchor.quote_asset_id)
  end

  # Members that trade nowhere at the quote (delisted) are dropped from the weights; their rows
  # still come along as exited holdings (fold_memberships!).
  def tradeable_weights
    weights.slice(*weights.keys.select { |id| venues.key?(id) })
  end

  # Exchanges that list every tradeable member at the quote.
  def common_exchange_ids
    sets = tradeable_weights.keys.map { |id| venues[id].to_set }
    sets.empty? ? [] : sets.reduce(:&).to_a.sort
  end

  def unavailable = [:unavailable, { label: anchor.label }]

  # The merged walk against the sum of the sources' books, read fresh in `reason`, on the two figures
  # no merge may change: units per asset and net money in. Together they fix the total result at any
  # price; everything else the pool derives is free to differ. Holdings are compared by the asset
  # behind each key (`key_assets`), never by the key: two sources may each call a different asset
  # "ABC" while the merged walk tells them apart as "ABC#id". To 1e-12, below any quote's smallest unit.
  def verify_books!(merged)
    actual = merged.metrics(force: true)
    expected_units = Hash.new(0.to_d)
    @books.each_value { |data| units_by_identity(data).each { |identity, units| expected_units[identity] += units } }
    actual_units = units_by_identity(actual)
    units_ok = (expected_units.keys | actual_units.keys).all? do |identity|
      expected_units[identity].round(12) == actual_units[identity].round(12)
    end
    money_ok = @books.values.sum { |data| net_in(data) }.round(12) == net_in(actual).round(12)
    return if units_ok && money_ok

    # The walk just cached this id's books; the rollback that follows frees the id, and SQLite hands
    # it to the next bot, which would read them as its own for thirty days.
    Rails.cache.delete(merged.send(:metrics_cache_key))
    raise Interleaved
  end

  # Money in from outside less the money the books still hold, with every estimated sale's proceeds
  # added back — an estimate is the cost of what was sold, which a pooled history changes. A buy that
  # spends another source's proceeds moves both terms alike; money lost moves only one.
  def net_in(data)
    data[:total_quote_amount_invested].to_d - data[:rebalance_cash].to_d + data[:estimated_proceeds].to_d
  end

  # { asset id or, for a holding known only by its string, that string => units }. The string comes from
  # key_strings, never the key: beside an asset of the same name the merged walk keys it "ABC#?".
  def units_by_identity(data)
    key_assets = data[:key_assets] || {}
    key_strings = data[:key_strings] || {}
    (data[:asset_breakdown] || {}).each_with_object(Hash.new(0.to_d)) do |(key, entry), units|
      units[key_assets[key] || key_strings[key]&.first || key] += entry[:amount].to_d
    end
  end

  def fail!(reason)
    @error = self.class.translate(reason)
    nil
  end

  # Scheduled, ready and blocked ticks and limit checks. Nothing here can be claimed while the lease
  # is held; a queued row left behind would wake a deleted bot for nothing.
  def cancel_queued_jobs(source)
    source.cancel_scheduled_action_jobs
    source.cancel_scheduled_limit_check_jobs if source.respond_to?(:cancel_scheduled_limit_check_jobs)
  end

  def write!
    bot.settings['allocations'] = allocations
    bot.set_missed_quote_amount
    bot.save! # after_save refresh_composition writes one member row per allocation

    ids = bots.map(&:id)
    Transaction.where(bot_id: ids).update_all(bot_id: bot.id)
    BotActivityLog.where(bot_id: ids).update_all(bot_id: bot.id)
    fold_memberships!
    bot.update_columns(redeploy_declined_offset: bots.sum(&:redeploy_declined_offset))
    resolved = bots.flat_map(&:resolved_liquidation_order_ids).uniq
    # What the merged bot inherited, so its own first tick is still a first tick (own_transactions),
    # and the sales its sources' users already accounted for stay accounted for.
    bot.merge_transient_data!(Bot::Composition::OrderSetter::MERGED_HISTORY_KEY => Transaction.where(bot_id: bot.id).maximum(:id),
                              Bot::LiquidationState::RESOLVED_KEY => resolved.presence)
    Bot.where(id: ids).update_all(status: Bot.statuses[:deleted], stopped_at: Time.current)
    bot.log_activity('merged', details: { source_ids: ids, source_labels: bots.map(&:label) })
    bot
  end

  # One row per asset on the merged bot. A member's row already exists (refresh_composition wrote
  # it): keep it, remember the earliest entry. Anything else with a row — an exited member, a member
  # dropped from the weights because its ticker no longer trades — comes along as an exited row, which
  # is what keeps its holding priced, charted and sellable under "Removed from portfolio". The row
  # points at the merged bot's venue's ticker for the asset when there is one; a source row from
  # another venue keeps its own ticker otherwise, which prices nothing here, as a delisting would.
  # Source rows are left where they are, as every other deleted bot's rows are.
  def fold_memberships!
    now = Time.current
    local_tickers = Ticker.where(exchange_id: exchange.id, quote_asset_id: anchor.quote_asset_id)
                          .pluck(:base_asset_id, :id).to_h
    BotIndexAsset.where(bot_id: bots.map(&:id)).order(:id).find_each do |row|
      existing = bot.bot_index_assets.find_by(asset_id: row.asset_id)
      if existing
        earliest = [existing.entered_at, row.entered_at].compact.min
        existing.update_columns(entered_at: earliest) if earliest && earliest != existing.entered_at
      else
        bot.bot_index_assets.create!(asset_id: row.asset_id, ticker_id: local_tickers[row.asset_id] || row.ticker_id,
                                     in_index: false, entered_at: row.entered_at, exited_at: row.exited_at || now,
                                     target_allocation: row.target_allocation,
                                     current_allocation: row.current_allocation)
      end
    end
  end
end
