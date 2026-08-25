# The history the nightly sync cannot know: one forward sweep over the whole ledger, from the first
# transaction to yesterday, valuing what was held on each day at that day's price.
#
# One price-range fetch per symbol, over the interval it was actually held — not per day and not per
# row. A hole inside a symbol's history carries the last observed price forward; a day BEFORE its
# first observed price, or a symbol with no price at all, leaves the day `partial` rather than
# valuing the holding at zero.
#
# Idempotent: rerunning upserts the same rows, so a failed run costs nothing.
class PortfolioSnapshot::BackfillJob < ApplicationJob
  queue_as :low_priority
  limits_concurrency to: 1, key: ->(user_id, exchange_id = nil) { "portfolio_backfill_#{user_id}_#{exchange_id}" },
                     on_conflict: :discard

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

  # With an exchange, the same sweep over that venue's rows alone, cached for the chart to read;
  # without one, the whole portfolio, written to the table the nightly sync appends to.
  def perform(user_id, exchange_id = nil)
    # Fiat cash is valued straight off the ECB table, and a history of nothing but cash never builds
    # a price service — which is the only other thing that loads it.
    Tax::EcbFxRates.ensure_loaded!
    @user = User.find(user_id)
    @exchange = exchange_id && Exchange.find(exchange_id)
    @transactions = transactions_in_scope
    return if @transactions.empty?

    @last_date = Date.current - 1
    first_date = @transactions.first.transacted_at.to_date
    return if first_date > @last_date

    load_prices(first_date)
    @exchange ? cache_series(sweep(first_date)) : store_history(sweep(first_date))
  end

  private

  def transactions_in_scope
    scope = AccountTransaction.for_user(@user).by_date_asc.includes(:exchange, :linked_transaction, :inverse_link)
    scope = scope.for_exchange(@exchange) if @exchange
    scope.to_a
  end

  def store_history(rows)
    PortfolioSnapshot.upsert_all(rows, unique_by: %i[user_id date], record_timestamps: true)
    # Stamped AFTER the sweep, so a price this run fetched itself counts as already read.
    PortfolioSnapshot.mark_prices_swept!(@user)
    warm_ledgers
  end

  # Today closes the scoped series the way `record!` closes the stored one: from the balances and
  # the ledger, because the price table has not closed today yet.
  def cache_series(rows)
    today = PortfolioSnapshot.today_row(@user, exchange: @exchange)
    rows << today if today
    PortfolioSnapshot.cache_series(@user, @exchange, rows)
    Turbo::StreamsChannel.broadcast_refresh_to("user_#{@user.id}", :sync)
  end

  # One row per day. Transactions are applied as their day comes round, then the balances standing
  # at the end of it are valued.
  #
  # Money in is not worked out here: it is the ledger's figure, read term by term in the ledger's
  # own order (`Tracker::Ledger.money_in`) and summed up to each day — so the history's last point
  # and the tile are one number, and there is no second opinion about what a row contributed. The
  # sweep keeps only what VALUING a day needs: the quantities, and the cash a venue must have had
  # to pay for what it bought.
  def sweep(first_date)
    balances = Hash.new(0.to_d)
    # Cash per VENUE, beside the balances the day is valued from: dollars at a broker cannot pay for
    # an exchange's trade, and a broker's own deficit is borrowed rather than missing. Both readers
    # of the ledger enumerate the moves with `UnfundedCash.moves`, so neither can hold a second
    # opinion about what a row does to cash.
    cash = Hash.new(0.to_d)
    closers = event_closers
    pending = @transactions.dup
    terms = Tracker::Ledger.money_in(@user, exchange: @exchange)
    invested = 0.to_d
    # A term nobody could state in full leaves the money-in figure an estimate from that day on.
    invested_incomplete = false

    (first_date..@last_date).map do |date|
      while pending.first && pending.first.transacted_at.to_date <= date
        transaction = pending.shift
        apply(balances, transaction)
        cash_moves(transaction).each { |currency, amount| cash[[transaction.exchange.name_id, currency]] += amount }
        venue = closers[transaction.id]
        unfunded_on(cash, balances, venue) if venue
      end
      while terms.first && terms.first.at.to_date <= date
        term = terms.shift
        invested += term.amount
        invested_incomplete ||= !term.complete
      end
      value, unpriced = value_on(balances, date)
      { user_id: @user.id, date: date, value_usd: value, invested_usd: invested,
        partial: unpriced || invested_incomplete }
    end
  end

  # Quantities move exactly as the tax engines move them, fees included: a fee in the asset being
  # acquired only shrinks what arrived, a fee in a third asset leaves that asset, and a fee in the
  # asset being SOLD is not taken off again — the adapters already report those sales net.
  def apply(balances, transaction)
    symbol = transaction.base_currency
    amount = transaction.base_amount.to_d

    case transaction.entry_type.to_sym
    when *ACQUISITIONS
      balances[symbol] += acquired(transaction, amount)
    when :deposit
      # A linked deposit is the far end of the user's own transfer: the withdrawal never removed
      # the coins, so this leg adds nothing.
      balances[symbol] += acquired(transaction, amount) unless transaction.linked?
    when :sell, :swap_out
      balances[symbol] -= amount
    when :withdrawal
      balances[symbol] -= transaction.linked? ? network_fee(transaction) : amount
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
      balances[symbol] += shortfall
    end
  end

  # A negative balance is history we do not have — an exchange whose ledger window starts after the
  # funding deposit leaves a sale with nothing behind it. Dropping it silently would show the whole
  # position as profit, so the day says it is an estimate instead.
  def value_on(balances, date)
    unpriced = balances.any? { |_symbol, quantity| quantity < -DUST }
    total = balances.sum(0.to_d) do |symbol, quantity|
      next 0.to_d unless quantity.positive?

      value = if STABLECOINS.include?(symbol)
                quantity
              elsif FIAT.include?(symbol)
                fiat_value(symbol, quantity, date)
              else
                price = @prices.dig(symbol, date)
                price && (quantity * price)
              end
      unpriced ||= value.nil?
      value || 0.to_d
    end
    [total, unpriced]
  end

  def fiat_value(currency, amount, date)
    amount * Tax::EcbFxRates.rate(from: currency, to: 'USD', date: date)
  rescue Tax::EcbFxRates::MissingRate
    nil
  end

  # symbol → { date => price }, last observed carried forward. Built once, over the interval each
  # symbol was actually touched, so a coin bought last week costs one small window rather than the
  # whole history.
  def load_prices(first_date)
    @prices = {}
    touched(first_date).each do |symbol, (from, exchange)|
      # A stock is a stock only on a venue that trades them: a coin sharing a ticker with a stock is
      # priced as the coin on a crypto venue.
      stock = Asset.find_by(symbol: symbol, category: STOCK_CATEGORIES) if exchange.stock_venue?
      key = stock ? "stock:#{symbol}" : symbol
      fetch_missing(symbol, stock, exchange, from)
      observed = HistoricalPrice.where(asset: key, currency: 'USD', date: from..@last_date)
                                .pluck(:date, :price).to_h
      last = nil
      carried = 0
      @prices[symbol] = (from..@last_date).index_with do |date|
        # ponytail: `stock_price_range` makes ONE candle request and Alpaca pages bars, so a stock
        # history longer than a page comes back truncated. The carry limit turns that into an
        # honest gap rather than a price repeated forever; paginating `get_bars` would remove it.
        carried = observed[date] ? 0 : carried + 1
        last = observed[date] || last
        last if carried <= CARRY_LIMIT
      end
    end
  end

  # Cash needs no price, and a symbol nothing ever touched needs no window. The venue that first
  # touched a symbol is the one its coin is read off (`Tax::AssetIdentity`).
  def touched(first_date)
    dates = {}
    @transactions.each do |transaction|
      date = [transaction.transacted_at.to_date, first_date].max
      [transaction.base_currency, transaction.quote_currency, transaction.fee_currency].compact.each do |symbol|
        next if FIAT.include?(symbol) || STABLECOINS.include?(symbol)

        dates[symbol] ||= [date, transaction.exchange]
      end
    end
    dates
  end

  # One range per coin, and only when the table does not already cover it — both fetchers check
  # that for themselves. A symbol that changed coin is two ranges; a symbol nobody can name a coin
  # for has nowhere to fetch from and stays unpriced.
  def fetch_missing(symbol, stock, exchange, from)
    if stock
      key = @user.api_keys.includes(:exchange).find { |api_key| api_key.exchange.tickers.exists?(base: symbol) }
      return unless key

      price_service.stock_price_range(exchange: key.exchange, api_key: key, symbol: symbol,
                                      from: from, to: @last_date)
    else
      Tax::AssetIdentity.coin_ids_over(symbol, exchange: exchange, from: from, to: @last_date).each do |range, coin_id|
        price_service.fetch_price_range(coin_id: coin_id, symbol: symbol, currency: 'USD',
                                        from: range.begin, to: range.end)
      end
    end
  end

  def price_service
    @price_service ||= Tax::PriceService.new
  end

  # The prices that just arrived are the ones every cached ledger was missing, so each scope gets
  # another chance at a complete figure.
  def warm_ledgers
    Tracker::LedgerJob.perform_later(@user.id)
    @user.api_keys.distinct.pluck(:exchange_id).each { |id| Tracker::LedgerJob.perform_later(@user.id, id) }
  end
end
