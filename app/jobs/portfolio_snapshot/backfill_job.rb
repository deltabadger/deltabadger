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
  limits_concurrency to: 1, key: ->(user_id) { "portfolio_backfill_#{user_id}" }, on_conflict: :discard

  FIAT = Tax::PriceService::FIAT_CURRENCIES
  STABLECOINS = Tax::PriceService::STABLECOINS
  # Categories whose prices live under the `stock:` namespace and come from the broker's own candles.
  STOCK_CATEGORIES = ['Stock', 'Common Stock', 'ETF', 'Fund'].freeze
  ACQUISITIONS = %i[buy swap_in staking_reward lending_interest airdrop mining other_income].freeze

  def perform(user_id)
    @user = User.find(user_id)
    @transactions = AccountTransaction.for_user(@user).by_date_asc.includes(:linked_transaction, :inverse_link).to_a
    return if @transactions.empty?

    @last_date = Date.current - 1
    first_date = @transactions.first.transacted_at.to_date
    return if first_date > @last_date

    load_prices(first_date)
    PortfolioSnapshot.upsert_all(sweep(first_date), unique_by: %i[user_id date], record_timestamps: true)
    warm_ledgers
  end

  private

  # One row per day. Transactions are applied as their day comes round, then the balances standing
  # at the end of it are valued.
  def sweep(first_date)
    balances = Hash.new(0.to_d)
    invested = 0.to_d
    # An unpriced deposit or withdrawal leaves the money-in figure wrong from that day on, not just
    # on the day itself.
    invested_incomplete = false
    pending = @transactions.dup

    (first_date..@last_date).map do |date|
      while pending.first && pending.first.transacted_at.to_date <= date
        transaction = pending.shift
        apply(balances, transaction)
        delta = contribution(transaction)
        delta.nil? ? invested_incomplete = true : invested += delta
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
    when :fee
      balances[symbol] -= amount
    when :adjustment
      balances[symbol] += amount # a split contributes only its signed net delta
    end
    consume_fee(balances, transaction)
    apply_quote(balances, transaction)
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
    return if transaction.fee_currency == transaction.base_currency

    balances[transaction.fee_currency] -= fee
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

  # Money in from outside, valued on the day it moved and never revalued after. nil means the day it
  # landed had no price for it — the figure cannot be stated, so the row says so.
  def contribution(transaction)
    direction = case transaction.entry_type.to_sym
                when :deposit then 1
                when :withdrawal then -1
                else return 0.to_d
                end
    return 0.to_d if transaction.linked?

    symbol = transaction.base_currency
    amount = transaction.base_amount.to_d
    date = transaction.transacted_at.to_date
    return direction * amount if STABLECOINS.include?(symbol)
    return fiat_value(symbol, amount, date)&.*(direction) if FIAT.include?(symbol)

    price = @prices.dig(symbol, date)
    price && (direction * amount * price)
  end

  def value_on(balances, date)
    unpriced = false
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
    touched(first_date).each do |symbol, from|
      asset = Asset.find_by(symbol: symbol)
      key = STOCK_CATEGORIES.include?(asset&.category) ? "stock:#{symbol}" : symbol
      fetch_missing(symbol, asset, from)
      observed = HistoricalPrice.where(asset: key, currency: 'USD', date: from..@last_date)
                                .pluck(:date, :price).to_h
      last = nil
      @prices[symbol] = (from..@last_date).index_with { |date| last = observed[date] || last }
    end
  end

  # Cash needs no price, and a symbol nothing ever touched needs no window.
  def touched(first_date)
    dates = {}
    @transactions.each do |transaction|
      date = [transaction.transacted_at.to_date, first_date].max
      [transaction.base_currency, transaction.quote_currency, transaction.fee_currency].compact.each do |symbol|
        next if FIAT.include?(symbol) || STABLECOINS.include?(symbol)

        dates[symbol] ||= date
      end
    end
    dates
  end

  # One range per symbol, and only when the table does not already cover it — both fetchers check
  # that for themselves. A symbol with no asset row has nowhere to fetch from and stays unpriced.
  def fetch_missing(symbol, asset, from)
    if STOCK_CATEGORIES.include?(asset&.category)
      key = @user.api_keys.includes(:exchange).find { |api_key| api_key.exchange.tickers.exists?(base: symbol) }
      return unless key

      price_service.stock_price_range(exchange: key.exchange, api_key: key, symbol: symbol,
                                      from: from, to: @last_date)
    elsif asset&.external_id.present?
      price_service.fetch_price_range(coin_id: asset.external_id, symbol: symbol, currency: 'USD',
                                      from: from, to: @last_date)
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
