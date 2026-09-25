# A multi-asset bot into one single-asset bot per member, each owning its asset's history.
#
# The user picks the bots on the dashboard. Each member that still trades on the bot's venue at its quote
# becomes a stopped one-asset basket on that venue, spending the bot's amount times the member's weight
# (weights re-normalised over the survivors, so the total schedule is kept) at the bot's interval, every
# rule off — as Bot::Merge builds its bot. Every order of that asset moves to it; every other row —
# an exited or delisted holding, a legacy row with no asset — moves to the first (heaviest) child, with
# the exited memberships, so it stays priced and sellable under "Removed from portfolio". The source is
# then soft-deleted, exactly as Merge leaves its sources.
#
# Moved rows are rewritten to REGULAR. A basket's history is cross-asset — a rebalance sell of one member
# funds a rebalance buy of another, a liquidation funds a redeploy — and cut by asset those rows would
# leave one child holding flight cash forever and another holding units with no cost. As REGULAR, a sale
# is money out and a buy is new money: the books of independent single-asset bots. Summed over the
# children, value − invested is the source's; invested grows by what the swaps moved. The one exception
# is a rebalance buy that overspent its sale: the source never booked the excess, a child books it as new
# money, which is the right reading. The rewrite, not a walk-time marker, so a child merged later keeps it.
#
# Money from past sales waits for the user's answer first: Yes redeploys it (the bot page's own redeploy),
# No keeps it as realised P/L (`keep_ids`). The modal asks; perform! refuses a bot still owed an answer.
#
# Three uses, one object: the tile asks `splittable?`, the modal asks `reason`, `offers`, `reinvesting?`,
# `halted?` and `assets`, and the POST calls `perform!`. The fence is Merge's: each venue's trading lock
# for the transaction, queued ticks cancelled after the commit.
class Bot::Split
  # Can this bot be picked at all? The tile's check; the modal's drops the redeploy states, which are
  # rows there rather than refusals.
  def self.splittable?(bot)
    eligible?(bot) && !bot.redeploy_in_flight?
  end

  def self.eligible?(bot)
    Bot::Merge::TYPES.include?(bot.type) && !bot.archived? && !bot.deleted? && !bot.executing? &&
      !bot.rebalance_pending? && !bot.liquidation_in_flight? && bot.bot_index_assets.in_index.count >= 2
  end

  attr_reader :user, :bots, :error

  # keep_ids: the bots whose sale proceeds the user chose to keep rather than reinvest.
  def initialize(user, ids, keep_ids: [])
    @user = user
    @ids = Array(ids).map(&:to_i).uniq
    @keep_ids = Array(keep_ids).map(&:to_i)
    load_bots
  end

  # An i18n key under errors.bots.split — with its interpolation when it names a bot — or nil when the
  # bots can be split once their proceeds are answered. An id that is gone refuses the whole split.
  def reason
    return :missing if bots.size < @ids.size
    return :none if bots.empty?

    unavailable = bots.find { |bot| !self.class.eligible?(bot) }
    return [:unavailable, { label: unavailable.label }] if unavailable

    # Both re-read the history in a way these rows' rewrite would change: a sale of units never bought
    # realises differently as REGULAR, and an unpriced sale is estimated at a basis the rewrite moves.
    external = bots.find { |bot| books(bot)[:external_sales] }
    return [:external_sales, { label: external.label }] if external

    unpriced = bots.find { |bot| books(bot)[:estimated_proceeds].to_d.positive? }
    return [:unpriced_sales, { label: unpriced.label }] if unpriced

    empty = bots.find { |bot| members(bot).empty? }
    return [:nothing_to_buy, { label: empty.label }] if empty

    nil
  end

  def refusal
    reason && Bot::Merge.translate(reason, scope: :split)
  end

  # { bot => proceeds on offer } — the "Redeploy N?" figure of the bot page.
  def offers
    @offers ||= bots.index_with { |bot| bot.redeploy_offer(books(bot)) }
  end

  # A Yes the user gave: queued (Solid Queue still holds the job) or placing.
  def reinvesting?(bot)
    bot.redeploy_in_flight? || bot.send(:active_job?, job_class: 'Bot::RedeployJob', record: bot)
  end

  def halted?(bot) = bot.redeploy_ambiguous?

  # The assets the children will buy, each once, in pick then weight order.
  def assets
    bots.flat_map { |bot| members(bot).map(&:first) }.uniq
  end

  # @return [Array<Bots::DcaMultiAsset>, nil] the children, or nil with #error set
  def perform!
    return fail!(reason) if reason
    return fail!(unanswered) if unanswered

    venues = bots.map(&:exchange).uniq
    children = nil
    held = Bot::VenueLease.hold(venues, holder: "Bot::Split for #{@ids.inspect}") do
      Bot.transaction do
        load_bots(lock: true)
        return fail!(reason) if reason
        return fail!(unanswered) if unanswered
        return fail!(unavailable) unless bots.all? { |source| venues.any? { |venue| venue.id == source.exchange_id } }

        children = bots.flat_map { |source| split!(source) }
      end
      bots.each do |source|
        source.cancel_scheduled_action_jobs
        source.cancel_scheduled_limit_check_jobs if source.respond_to?(:cancel_scheduled_limit_check_jobs)
      end
    end
    return fail!(unavailable) unless held

    children
  rescue ActiveRecord::RecordInvalid
    fail!(unavailable)
  end

  private

  def load_bots(lock: false)
    scope = user.bots.not_deleted.where(id: @ids)
    scope = scope.lock if lock
    by_id = scope.index_by(&:id)
    @bots = @ids.filter_map { |id| by_id[id] }
    @books = {}
    @members = {}
    @offers = nil
  end

  # Read fresh: a cached payload may predate the last fill.
  def books(bot)
    @books[bot.id] ||= bot.metrics(force: true)
  end

  # [[asset, weight]] the bot's members that still trade on its venue at its quote, weight order, the
  # weights re-normalised so the children together spend what the bot spent.
  def members(bot)
    @members[bot.id] ||= begin
      tradeable = Ticker.available.trading_enabled
                        .where(exchange_id: bot.exchange_id, quote_asset_id: bot.quote_asset_id)
                        .pluck(:base_asset_id).to_set
      pairs = bot.current_allocations.filter_map do |member|
        weight = member[:target_allocation].to_f
        [member[:asset], weight] if weight.positive? && tradeable.include?(member[:asset].id)
      end
      total = pairs.sum(&:last)
      pairs.map { |asset, weight| [asset, weight / total] }
    end
  end

  # The first bot whose proceeds still wait for an answer, as a refusal.
  def unanswered
    busy = bots.find { |bot| reinvesting?(bot) }
    return [:reinvesting, { label: busy.label }] if busy

    owed = bots.find { |bot| offers[bot].positive? && !@keep_ids.include?(bot.id) }
    [:proceeds, { label: owed.label }] if owed
  end

  def unavailable = [:unavailable, { label: bots.first&.label }]

  def fail!(reason)
    @error = Bot::Merge.translate(reason, scope: :split)
    nil
  end

  def split!(source)
    children = members(source).map { |asset, weight| [asset, build_child!(source, asset, weight)] }
    first = children.first.last

    # A legacy row recorded before orders stored their asset goes by its symbol — ours or the venue's
    # (Kraken's XBT) — or it would land on a child that holds no ticker to price it.
    children.each do |asset, child|
      spellings = [asset.symbol, *Ticker.where(exchange_id: source.exchange_id, base_asset_id: asset.id).pluck(:base)].uniq
      mine = Transaction.where(bot_id: source.id)
      mine.where(base_asset_id: asset.id).or(mine.where(base_asset_id: nil, base: spellings))
          .update_all(bot_id: child.id, transaction_type: 'REGULAR')
    end
    Transaction.where(bot_id: source.id).update_all(bot_id: first.id, transaction_type: 'REGULAR')
    move_memberships!(source, children.to_h { |asset, child| [asset.id, child] }, first)

    resolved = source.resolved_liquidation_order_ids.presence
    children.map(&:last).each do |child|
      child.merge_transient_data!(Bot::Composition::OrderSetter::MERGED_HISTORY_KEY => child.transactions.maximum(:id),
                                  Bot::LiquidationState::RESOLVED_KEY => resolved)
      child.log_activity('split', details: { source_id: source.id, source_label: source.label })
    end
    source.update_columns(status: Bot.statuses[:deleted], stopped_at: Time.current)
    children.map(&:last)
  end

  # As Bot::Merge#bot: settings first, then the exchange, whose limit-orders decorator writes into them.
  def build_child!(source, asset, weight)
    child = user.bots.new(type: 'Bots::DcaMultiAsset', status: :stopped, position: source.position,
                          settings: { 'quote_asset_id' => source.quote_asset_id,
                                      'quote_amount' => source.quote_amount.to_f * weight,
                                      'interval' => source.interval,
                                      'weighting' => 'manual',
                                      'allocations' => { asset.id.to_s => 1.0 } })
    child.exchange = source.exchange
    child.set_missed_quote_amount
    child.save! # after_save refresh_composition writes the member row
    child
  end

  # Each child's member row keeps the source's entry date; every other source row lands on the first
  # child as an exited row, as Bot::Merge#fold_memberships! folds them.
  def move_memberships!(source, child_for, first)
    now = Time.current
    BotIndexAsset.where(bot_id: source.id).order(:id).find_each do |row|
      if (child = child_for[row.asset_id])
        member = child.bot_index_assets.find_by(asset_id: row.asset_id)
        member&.update_columns(entered_at: row.entered_at) if row.entered_at
      elsif !first.bot_index_assets.exists?(asset_id: row.asset_id)
        first.bot_index_assets.create!(asset_id: row.asset_id, ticker_id: row.ticker_id, in_index: false,
                                       entered_at: row.entered_at, exited_at: row.exited_at || now,
                                       target_allocation: row.target_allocation,
                                       current_allocation: row.current_allocation)
      end
    end
  end
end
