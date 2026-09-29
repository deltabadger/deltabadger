# The history the nightly sync cannot know: one forward sweep over the whole ledger, from the first
# transaction to yesterday, valuing what was held on each day at that day's price — on every venue,
# and the whole as their sum.
#
# One price-range fetch per instrument, over the interval it was actually held — not per day and not
# per row. A hole inside a symbol's history carries the last observed price forward; a day BEFORE its
# first observed price, or a symbol with no price at all, leaves the day `partial` rather than
# valuing the holding at zero.
#
# Idempotent: rerunning upserts the same rows, so a failed run costs nothing.
class PortfolioSnapshot::BackfillJob < ApplicationJob
  queue_as :low_priority
  # Per user: one sweep writes every venue. The second argument is what a venue-scoped run was
  # enqueued with before that, still read so a job already in the queue runs.
  limits_concurrency to: 1, key: ->(user_id, *) { "portfolio_backfill_#{user_id}" }, on_conflict: :discard

  FIAT = Tax::PriceService::FIAT_CURRENCIES
  STABLECOINS = Tax::PriceService::STABLECOINS
  # Categories whose prices live under the `stock:` namespace and come from the broker's own candles.
  STOCK_CATEGORIES = ['Stock', 'Common Stock', 'ETF', 'Fund'].freeze
  ACQUISITIONS = %i[buy swap_in staking_reward lending_interest airdrop mining other_income].freeze
  # How far a last-observed price may be carried. A weekend and a long holiday fit inside it; a
  # broker page limit or a dead feed does not, and those days say so rather than repeating a price
  # from another market.
  CARRY_LIMIT = 7
  # Below this a negative balance is the adapters disagreeing about gross and net, not a hole.
  DUST = '0.00000001'.to_d

  def perform(user_id, _exchange_id = nil)
    # Fiat cash is valued straight off the ECB table, and a history of nothing but cash never builds
    # a price service — which is the only other thing that loads it.
    Tax::EcbFxRates.ensure_loaded!
    @user = User.find(user_id)
    # Read before the rows are, so a row landing mid-sweep leaves the history stale.
    version = PortfolioSnapshot.history_version(@user)
    @transactions = AccountTransaction.for_user(@user).by_date_asc
                                      .includes(:exchange, :inverse_link, linked_transaction: :exchange).to_a
    @last_date = Date.current - 1
    first_date = @transactions.first&.transacted_at&.to_date
    return PortfolioSnapshot.mark_history_swept!(@user, version) if first_date.nil? || first_date > @last_date

    load_prices(first_date)
    store(*sweep(first_date), version)
  end

  private

  # Both tables and the version they were swept from, together: a history is never half written,
  # nor stamped current for rows it did not read.
  def store(whole, venues, version)
    ActiveRecord::Base.transaction do
      PortfolioSnapshot.upsert_all(whole, unique_by: %i[user_id date], record_timestamps: true)
      PortfolioVenueSnapshot.upsert_all(venues, unique_by: %i[user_id exchange_id date], record_timestamps: true) if venues.any?
      PortfolioSnapshot.mark_history_swept!(@user, version)
    end
    # Stamped AFTER the sweep, so a price this run fetched itself counts as already read.
    PortfolioSnapshot.mark_prices_swept!(@user)
    # The prices that just arrived are the ones the cached ledger was missing, so it gets another
    # chance at a complete figure — and refreshes whoever is looking.
    Tracker::LedgerJob.perform_later(@user.id)
  end

  # One row per venue per day, and the whole as their sum. Transactions are applied as their day
  # comes round, then the balances standing at the end of it are valued.
  #
  # Money in is not worked out here: it is the ledger's figure, read term by term in the ledger's
  # own order (`Tracker::Ledger.money_in`) and summed up to each day — so the history's last point
  # and the tile are one number, and there is no second opinion about what a row contributed. The
  # sweep keeps only what VALUING a day needs: the quantities, and the cash a venue must have had
  # to pay for what it bought.
  #
  # Each day is written TWICE over, because the page has two readings of it and "Show cash" picks
  # between them: the whole portfolio, and the same day with the cash taken off both sides — the
  # value it stands in, and the money in that funds it. Both come off the day already swept; the
  # cash of the day is the one extra figure, and the sweep is already valuing it.
  def sweep(first_date)
    # Quantities per VENUE — a coin is valued on the venue that holds it, and the lots moved by a
    # linked transfer move here at the same instant.
    balances = Hash.new { |venues, venue| venues[venue] = Hash.new(0.to_d) }
    # Cash per VENUE, beside the balances the day is valued from: dollars at a broker cannot pay for
    # an exchange's trade, and a broker's own deficit is borrowed rather than missing. Both readers
    # of the ledger enumerate the moves with `UnfundedCash.moves`, so neither can hold a second
    # opinion about what a row does to cash.
    cash = Hash.new(0.to_d)
    closers = event_closers
    pending = @transactions.dup
    terms = Tracker::Ledger.money_in(@user)
    invested = Hash.new(0.to_d)
    # A term nobody could state in full leaves the money-in figure an estimate from that day on.
    incomplete = Hash.new(false)
    ids = Exchange.all.to_h { |exchange| [exchange.name_id, exchange.id] }
    # A venue's days start at its first row — or at the first transfer sent to it, which lands there
    # at the withdrawal.
    opened = @transactions.each_with_object({}) do |transaction, first|
      [transaction.exchange, transaction.linked_transaction&.exchange].compact.each do |exchange|
        first[exchange.name_id] ||= transaction.transacted_at.to_date
      end
    end

    whole = []
    venues = []
    (first_date..@last_date).each do |date|
      while pending.first && pending.first.transacted_at.to_date <= date
        transaction = pending.shift
        apply(balances, transaction)
        cash_moves(transaction).each { |currency, amount| cash[[transaction.exchange.name_id, currency]] += amount }
        venue = closers[transaction.id]
        unfunded_on(cash, balances, venue) if venue
      end
      while terms.first && terms.first.at.to_date <= date
        term = terms.shift
        invested[term.exchange] += term.amount
        incomplete[term.exchange] ||= !term.complete
        # An opening balance is held from the day the ledger booked it, as the ledger holds it.
        balances[term.exchange][term.opens.first] += term.opens.last if term.opens
      end
      days = (balances.keys | invested.keys).map do |venue|
        value, held, unpriced = value_on(venue, balances[venue], date)
        { venue: venue, value_usd: value, invested_usd: invested[venue], held_value_usd: held,
          held_cost_usd: invested[venue] - (value - held), partial: unpriced || incomplete[venue] }
      end
      whole << day(date, days)
      days.each do |row|
        next unless (since = opened[row[:venue]]) && date >= since && ids[row[:venue]]

        venues << row.except(:venue).merge(user_id: @user.id, exchange_id: ids[row[:venue]], date: date)
      end
    end
    [whole, venues]
  end

  def day(date, venues)
    sum = ->(figure) { venues.sum(0.to_d) { |row| row[figure] } }
    { user_id: @user.id, date: date, value_usd: sum.call(:value_usd), invested_usd: sum.call(:invested_usd),
      held_value_usd: sum.call(:held_value_usd), held_cost_usd: sum.call(:held_cost_usd),
      partial: venues.any? { |row| row[:partial] } }
  end

  # Quantities move exactly as the tax engines move them, fees included: a fee in the asset being
  # acquired only shrinks what arrived, a fee in a third asset leaves that asset, and a fee in the
  # asset being SOLD is not taken off again — the adapters already report those sales net.
  def apply(venues, transaction)
    symbol = transaction.base_currency
    amount = transaction.base_amount.to_d
    balances = venues[transaction.exchange.name_id]

    case transaction.entry_type.to_sym
    when *ACQUISITIONS
      balances[symbol] += acquired(transaction, amount)
    when :deposit
      # A linked deposit is the far end of the user's own transfer: the withdrawal already moved
      # the coins, so this leg adds nothing.
      balances[symbol] += acquired(transaction, amount) unless linked?(transaction)
    when :sell, :swap_out
      balances[symbol] -= amount
    when :withdrawal
      withdraw(venues, balances, transaction, amount)
    when :fee, :lost
      balances[symbol] -= amount
    when :withholding_tax
      # Inert in the tax engines, which track holdings; here it is cash the broker kept, and cash
      # that never leaves overstates every day after it.
      balances[symbol] -= amount
    when :adjustment
      balances[symbol] += amount # a split contributes only its signed net delta
    end
    consume_fee(balances, transaction)
    apply_quote(balances, transaction)
  end

  # The transactions a shortfall may be read at, by id — see `UnfundedCash.closers`.
  def event_closers
    closing = Tracker::UnfundedCash.closers(@transactions.map { |t| [t.exchange.name_id, t.group_id] })
    @transactions.each_with_index.with_object({}) do |(transaction, index), closers|
      closers[transaction.id] = closing[index] if closing[index]
    end
  end

  def cash_moves(transaction)
    return [] if Tracker::UnfundedCash.borrowed?(transaction.tx_id)

    Tracker::UnfundedCash.moves(**transaction.slice(*Tracker::UnfundedCash::MOVE_KEYS).symbolize_keys)
  end

  def acquired(transaction, amount)
    return amount unless transaction.fee_currency == transaction.base_currency && transaction.fee_amount.present?

    [amount - transaction.fee_amount.to_d, 0.to_d].max
  end

  # Every fee that is NOT in the base asset leaves its own currency — a third crypto asset out of
  # that asset, a quote or fiat fee out of the cash the trade settled in.
  def consume_fee(balances, transaction)
    fee = transaction.fee_amount.to_d
    return unless fee.positive? && transaction.fee_currency.present?
    return if transaction.fee_currency == transaction.base_currency && !cash_leg?(transaction)

    balances[transaction.fee_currency] -= fee
  end

  # A venue that books each leg of a trade as its own row charges the fee on the cash leg, on top of
  # the amount it reports: the "already net" rule above is about the asset being sold, and cash is
  # not being sold — it is paying. Reading it the other way leaves the account holding money it has
  # already spent, and disagreeing with the ledger about how much came in to spend it.
  def cash_leg?(transaction)
    return false unless Tracker::UnfundedCash::FEE_ON_TOP.include?(transaction.entry_type.to_s)
    return false if transaction.quote_currency.present? # a trade with its own quote reports it net

    Tracker::UnfundedCash.cash?(transaction.base_currency)
  end

  # The cash side of a single-row trade. Kraken books each leg as its own row with no quote, so
  # nothing double-counts there.
  def apply_quote(balances, transaction)
    quote = transaction.quote_currency
    amount = transaction.quote_amount
    return if quote.blank? || amount.blank?

    case transaction.entry_type.to_sym
    when :buy then balances[quote] -= amount.to_d
    when :sell, :return_of_capital then balances[quote] += amount.to_d
    end
  end

  def linked?(transaction)
    (transaction.linked_transaction || transaction.inverse_link).present?
  end

  # Unlinked, the coins left the account. Linked, the network fee leaves and the rest lands on the
  # venue it was sent to, at the withdrawal — the instant the ledger moves the lots.
  def withdraw(venues, balances, transaction, amount)
    return balances[transaction.base_currency] -= amount unless linked?(transaction)

    fee = network_fee(transaction)
    destination = transaction.linked_transaction.exchange.name_id
    if destination == transaction.exchange.name_id
      balances[transaction.base_currency] -= fee
    else
      balances[transaction.base_currency] -= amount
      venues[destination][transaction.base_currency] += amount - fee
    end
  end

  def network_fee(withdrawal)
    [withdrawal.base_amount.to_d - withdrawal.linked_transaction.base_amount.to_d, 0.to_d].max
  end

  # Cash the ledger spent without ever seeing it arrive: the coins were bought with money whatever
  # the venue reported, so the account really did hold it. Added back to both the pot and the
  # balances, so a sale that returns it lands on a balance of zero rather than paying off a debt
  # that was never owed — and the day it was spent is valued with it present. What it adds to
  # money in is the ledger's term for that row, not a figure of this sweep's own.
  def unfunded_on(cash, balances, venue)
    return if Tracker::UnfundedCash.lends_cash?(venue)

    cash.each do |(exchange, symbol), balance|
      next unless exchange == venue

      shortfall = Tracker::UnfundedCash.shortfall(symbol, balance)
      next if shortfall.zero?

      cash[[exchange, symbol]] += shortfall
      balances[venue][symbol] += shortfall
    end
  end

  # [everything, the positions inside it, whether a holding went unpriced]. Cash is split out of the
  # same walk rather than filtered in a second one, so the two readings of a day cannot disagree
  # about it — and it is split by the predicate the page uses (`UnfundedCash.cash?`), so the curve
  # and the holdings card mean the same thing by "cash".
  #
  # A negative balance is history we do not have — an exchange whose ledger window starts after the
  # funding deposit leaves a sale with nothing behind it. Dropping it silently would show the whole
  # position as profit, so the day says it is an estimate instead.
  #
  # Cash below zero at a venue that lends it is not a hole but a debt — a margin buy — and is valued
  # as one: the whole account is what is held less what is owed, as it was when the venues' cash was
  # netted in one pot.
  def value_on(venue, balances, date)
    owed = ->(symbol, quantity) { quantity.negative? && Tracker::UnfundedCash.cash?(symbol) && Tracker::UnfundedCash.lends_cash?(venue) }
    unpriced = balances.any? { |symbol, quantity| quantity < -DUST && !owed.call(symbol, quantity) }
    total = 0.to_d
    held = 0.to_d
    balances.each do |symbol, quantity|
      next unless quantity.positive? || owed.call(symbol, quantity)

      value = if STABLECOINS.include?(symbol)
                quantity
              elsif FIAT.include?(symbol)
                fiat_value(symbol, quantity, date)
              else
                price = @prices.dig(@price_keys.fetch([venue, symbol], symbol), date)
                price && (quantity * price)
              end
      unpriced ||= value.nil?
      total += value || 0.to_d
      held += value || 0.to_d unless Tracker::UnfundedCash.cash?(symbol)
    end
    [total, held, unpriced]
  end

  def fiat_value(currency, amount, date)
    amount * Tax::EcbFxRates.rate(from: currency, to: 'USD', date: date)
  rescue Tax::EcbFxRates::MissingRate
    nil
  end

  # price key → { date => price }, last observed carried forward. Built once per INSTRUMENT, over the
  # interval it was actually touched, so a coin bought last week costs one small window rather than
  # the whole history. A [venue, symbol] names its instrument (`@price_keys`): a stock is a stock only
  # on a venue that trades them — a coin sharing a ticker with a stock is priced as the coin on a
  # crypto venue, and both can be held at once.
  def load_prices(first_date)
    @prices = {}
    @price_keys = {}
    instruments = {}
    touched(first_date).each do |(venue, symbol), (from, exchange)|
      stock = Asset.find_by(symbol: symbol, category: STOCK_CATEGORIES) if exchange.stock_venue?
      key = stock ? "stock:#{symbol}" : symbol
      @price_keys[[venue, symbol]] = key
      # The venue that first touched a coin is the one its identity is read off (`Tax::AssetIdentity`).
      known = instruments[key]
      instruments[key] = [symbol, stock, [from, known&.dig(2) || from].min, known&.dig(3) || exchange]
    end
    instruments.each do |key, (symbol, stock, from, exchange)|
      coins = stock ? [] : Tax::AssetIdentity.coin_ids_over(symbol, exchange: exchange, from: from, to: @last_date)
      fetch_missing(symbol, stock, from, coins, exchange)
      observed = HistoricalPrice.where(asset: key, currency: 'USD', date: from..@last_date)
                                .pluck(:date, :price).to_h
      # A price carries over a hole only while the symbol still means the same coin: across the day
      # it changed coin (or stopped meaning any), the last price is another coin's — QUICK's old
      # token is ~1000x the new one.
      coin_on = ->(date) { stock ? key : coins.find { |days, _| days.cover?(date) }&.last }
      last = nil
      carried = 0
      @prices[key] = (from..@last_date).index_with do |date|
        last = nil if date > from && coin_on.call(date) != coin_on.call(date - 1)
        # ponytail: `stock_price_range` makes ONE candle request and Alpaca pages bars, so a stock
        # history longer than a page comes back truncated. The carry limit turns that into an
        # honest gap rather than a price repeated forever; paginating `get_bars` would remove it.
        carried = observed[date] ? 0 : carried + 1
        last = observed[date] || last
        last if carried <= CARRY_LIMIT
      end
    end
  end

  # Cash needs no price, and a symbol nothing ever touched needs no window. [venue, symbol] → the
  # first day it was touched there, and that venue. A linked transfer touches its destination too.
  def touched(first_date)
    dates = {}
    @transactions.each do |transaction|
      date = [transaction.transacted_at.to_date, first_date].max
      venues = [transaction.exchange, transaction.linked_transaction&.exchange].compact
      [transaction.base_currency, transaction.quote_currency, transaction.fee_currency].compact.each do |symbol|
        next if FIAT.include?(symbol) || STABLECOINS.include?(symbol)

        venues.each { |exchange| dates[[exchange.name_id, symbol]] ||= [date, exchange] }
      end
    end
    dates
  end

  # One range per coin, and only when the table does not already cover it — both fetchers check
  # that for themselves. A symbol that changed coin is two ranges; a symbol nobody can name a coin
  # for has nowhere to fetch from and stays unpriced.
  #
  # A stock's closes come from the venue that holds it, and only that one: `stock:SYM` is the table
  # the tax report prices the broker from too, and a crypto venue listing the same ticker would store
  # the coin's candles under the stock's name.
  def fetch_missing(symbol, stock, from, coins, exchange)
    if stock
      key = @user.api_keys.find_by(exchange_id: exchange.id)
      return unless key

      price_service.stock_price_range(exchange: key.exchange, api_key: key, symbol: symbol,
                                      from: from, to: @last_date)
    else
      coins.each do |range, coin_id|
        price_service.fetch_price_range(coin_id: coin_id, symbol: symbol, currency: 'USD',
                                        from: range.begin, to: range.end)
      end
    end
  end

  def price_service
    @price_service ||= Tax::PriceService.new
  end
end
