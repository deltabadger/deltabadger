module Tax
  class PriceService
    STABLECOINS = %w[USDT USDC BUSD DAI FDUSD TUSD PYUSD RLUSD].freeze
    # Settlement currencies of the venues this app syncs — Kraken books AUD, CAD, JPY and AED, and
    # every entry here is a row the engines must NOT treat as a crypto disposal. Only add a code a
    # venue actually settles in: a symbol listed here is dropped from the report, so a token sharing
    # a fiat ticker would vanish from a signed form.
    FIAT_CURRENCIES = %w[USD EUR GBP CHF SEK PLN DKK CZK BGN AUD CAD JPY AED].freeze
    # A Kraken trade books its fee on whichever leg paid it, in either direction, so the crypto leg
    # of a sale needs the same treatment as the crypto leg of a buy.
    TRADE_TYPES = %w[buy swap_in sell swap_out].freeze
    private_constant :TRADE_TYPES

    attr_reader :warnings

    def initialize
      Tax::EcbFxRates.ensure_loaded!
      @price_cache = {}
      @warnings = []
      @warned_fx_keys = Set.new
    end

    # Pre-fetches all needed prices in bulk: one API call per coin instead of per day.
    def prefetch(transactions, currency:, &on_progress)
      coins_needed = {}

      transactions.each do |tx|
        next if skip_pricing?(tx)
        next if tx.quote_currency == currency && tx.quote_amount.present?
        next if tx.quote_currency.present? && tx.quote_amount.present? &&
                (STABLECOINS.include?(tx.quote_currency) || FIAT_CURRENCIES.include?(tx.quote_currency))

        add_coin_date(coins_needed, tx.base_currency, tx.transacted_at)
        if tx.fee_amount.present? && tx.fee_amount.positive? && !STABLECOINS.include?(tx.fee_currency)
          add_coin_date(coins_needed, tx.fee_currency, tx.transacted_at)
        end
      end

      if currency != 'USD'
        all_dates = coins_needed.values.flat_map { |info| info[:dates].to_a }
        add_coin_dates(coins_needed, 'BTC', all_dates) if all_dates.any?
      end

      # Fetch each coin's full range in one API call
      fetchable = coins_needed.select { |sym, _| asset_to_coingecko_id(sym) }
      total_coins = fetchable.size
      fetchable.each_with_index do |(symbol, info), index|
        coin_id = asset_to_coingecko_id(symbol)
        Rails.logger.info("[TaxReport] Fetching prices for #{symbol} (#{index + 1}/#{total_coins})")
        fetch_price_range(coin_id: coin_id, symbol: symbol, currency: currency,
                          from: info[:min], to: info[:max])
        on_progress&.call(index + 1, total_coins)
      end
    end

    def price_at(asset:, currency:, timestamp:)
      return 1.to_d if asset == currency
      return stablecoin_rate(currency, timestamp) if STABLECOINS.include?(asset)

      cache_key = "#{asset}/#{currency}/#{timestamp.to_date}"
      cached = @price_cache[cache_key]
      # A zero is never a price, it is a failed lookup wearing one. `nil.to_d` is `0.0`, so a single
      # null in an upstream prices array becomes a zero here — and a zero that gets past this point
      # reports the entire proceeds as gain on a row marked complete. Treat it as missing wherever it
      # comes from, so an already-poisoned historical_prices row heals on the next report instead of
      # reproducing the wrong number forever.
      return cached if cached && !cached.to_d.zero?

      # Check DB
      db_price = HistoricalPrice.lookup(asset: asset, currency: currency, date: timestamp.to_date)
      if db_price && !db_price.to_d.zero?
        @price_cache[cache_key] = db_price
        return db_price
      end

      # Fetch from CoinGecko
      price = fetch_single_price(asset: asset, currency: currency, timestamp: timestamp)
      if price.nil? || price.zero?
        @warnings << "#{asset}/#{currency} #{timestamp.to_date}"
        return 0.to_d
      end
      price
    end

    def convert_fiat(amount:, from:, to:, timestamp:)
      return amount if from == to

      rate = fiat_exchange_rate(from: from, to: to, timestamp: timestamp)
      amount * rate
    end

    # Enriches transactions with fiat values for tax calculation.
    # Progress is split: 0-21% = prefetching prices, 21-100% = enriching transactions.
    def enrich(transactions, currency:, &on_progress)
      prefetch(transactions, currency: currency) do |done, total|
        percent = total.positive? ? (done.to_f / total * 21).to_i : 0
        on_progress&.call(percent, 100)
      end

      links = transactions.filter_map { |tx| [tx.id, tx.linked_transaction_id] if tx.linked_transaction_id }.to_h
      linked_deposit_ids = links.values.to_set
      deposit_amounts = AccountTransaction.where(id: links.values).pluck(:id, :base_amount).to_h
      total = transactions.size
      entries = transactions.each_with_index.map do |tx, index|
        warnings_before = @warnings.size
        fiat_value, fee_fiat_value = if skip_pricing?(tx)
                                       [0.to_d, 0.to_d]
                                     else
                                       [resolve_fiat_value(tx, currency), resolve_fee_fiat_value(tx, currency)]
                                     end
        price_missing = @warnings.size > warnings_before

        enrich_percent = total.positive? ? 21 + ((index + 1).to_f / total * 79).to_i : 100
        on_progress&.call(enrich_percent, 100)

        {
          entry_type: tx.entry_type,
          base_currency: tx.base_currency,
          base_amount: tx.base_amount.to_d,
          quote_currency: tx.quote_currency,
          quote_amount: tx.quote_amount&.to_d,
          raw_data: tx.raw_data,
          fiat_value: fiat_value,
          fee_fiat_value: fee_fiat_value,
          fee_currency: tx.fee_currency,
          fee_amount: tx.fee_amount&.to_d,
          transacted_at: tx.transacted_at,
          tx_id: tx.tx_id,
          group_id: tx.group_id,
          price_missing: price_missing,
          exchange: tx.exchange.name_id,
          linked: links.key?(tx.id) || linked_deposit_ids.include?(tx.id),
          transfer_fee_amount: transfer_fee_amount(tx, links, deposit_amounts)
        }
      end

      attribute_quote_row_fees(entries)
      entries
    end

    def stock_price_range(exchange:, api_key:, symbol:, from:, to:)
      # Stocks and crypto share this table, so the namespace prevents identical symbols from
      # silently borrowing one another's prices.
      asset = "stock:#{symbol}"
      db_prices = HistoricalPrice.where(asset: asset, currency: 'USD', date: from..to)

      # "Any row at all" would cache a truncated first fetch forever, and the caller takes the LAST
      # price in the window — so a window ending nine days early would quietly become a year-boundary
      # value in a Vorabpauschale. Keep fetching until the window reaches its last weekday.
      last_weekday = (from..to).reverse_each.find { |date| (1..5).cover?(date.wday) } || to
      if db_prices.where(date: last_weekday..).none?
        ticker = exchange.tickers.find_by(base: symbol)
        return {} unless ticker

        # A delisted ticker, an expired key or a broker outage must leave the symbol without a
        # boundary price — the broker report refuses that symbol rather than valuing it at zero.
        # Scoped to the network call on purpose: a wider rescue would launder a real bug into the
        # same silent refusal.
        result = begin
          exchange.set_client(api_key: api_key)
          exchange.get_candles(ticker: ticker, start_at: from.to_time(:utc), timeframe: 1.day)
        rescue StandardError
          return {}
        end
        return {} if result.failure?

        stored_dates = HistoricalPrice.where(asset: asset, currency: 'USD', date: from..to).pluck(:date).to_set
        records = result.data.filter_map do |candle|
          date = candle[0].to_date
          next unless (from..to).cover?(date)
          next if stored_dates.include?(date)

          stored_dates << date
          { asset: asset, currency: 'USD', date: date, price: candle[4].to_d }
        end
        HistoricalPrice.bulk_store(records)
        db_prices = HistoricalPrice.where(asset: asset, currency: 'USD', date: from..to)
      end

      db_prices.filter_map do |historical_price|
        usd_per_eur = Tax::EcbFxRates.rate(from: 'EUR', to: 'USD', date: historical_price.date)
        [historical_price.date, historical_price.price.to_d / usd_per_eur]
      rescue Tax::EcbFxRates::MissingRate
        nil
      end.to_h
    end

    private

    # Since transfers stopped being disposals, no engine reads a withdrawal's fiat value — the
    # FIFO family and PVCT branch on `linked` and `transfer_fee_amount`, both derived from base
    # amounts, and the wealth snapshot never sees enriched rows at all. Pricing one can therefore
    # only fail, and a failure would raise a missing-price warning that banners the whole report as
    # incomplete over a number nothing consumes — corroding the very signal that banner exists for.
    def skip_pricing?(transaction)
      transaction.withdrawal?
    end

    def add_coin_date(coins, symbol, timestamp)
      return if symbol.blank? || FIAT_CURRENCIES.include?(symbol)

      date = timestamp.to_date
      coins[symbol] ||= { min: date, max: date, dates: Set.new }
      coins[symbol][:min] = date if date < coins[symbol][:min]
      coins[symbol][:max] = date if date > coins[symbol][:max]
      coins[symbol][:dates] << date
    end

    def add_coin_dates(coins, symbol, dates)
      dates.each do |d|
        date = d.is_a?(Date) ? d : d.to_date
        add_coin_date(coins, symbol, date.to_datetime)
      end
    end

    def fetch_price_range(coin_id:, symbol:, currency:, from:, to:)
      # Load existing prices from DB first
      db_prices = HistoricalPrice.where(asset: symbol, currency: currency, date: from..to)
      db_prices.each do |hp|
        cache_key = "#{symbol}/#{currency}/#{hp.date}"
        @price_cache[cache_key] = hp.price
      end

      # Check if we already have all dates covered
      db_dates = db_prices.pluck(:date).to_set
      needed_dates = (from..to).to_a
      return if needed_dates.all? { |d| db_dates.include?(d) }

      # Fetch missing from MarketData (CoinGecko or data-api)
      result = MarketData.get_historical_price_range(
        coin_id: coin_id,
        currency: currency.downcase,
        from: from.to_time.beginning_of_day,
        to: (to + 1.day).to_time.beginning_of_day
      )

      return if result.failure?

      prices = result.data['prices']
      return if prices.blank?

      records_to_store = []
      prices.each do |timestamp_ms, price|
        # A null in the array arrives as nil and `nil.to_d` is 0 — neither cache nor store it, or the
        # missing-price guard in `price_at` never sees the gap and the row reports full proceeds as
        # gain. `insert_all` bypasses the presence validation, so this is the only place to stop it.
        next if price.to_d.zero?

        date = Time.at(timestamp_ms / 1000.0).utc.to_date
        cache_key = "#{symbol}/#{currency}/#{date}"
        next if @price_cache[cache_key] # already from DB

        @price_cache[cache_key] = price.to_d
        records_to_store << { asset: symbol, currency: currency, date: date, price: price.to_d }
      end

      HistoricalPrice.bulk_store(records_to_store)
    end

    def resolve_fiat_value(record, currency)
      # `prefetch`/`add_coin_date` already refuses to price a fiat base. Report drops fiat rows before
      # the engines, so nothing reads this value; pricing it can only fabricate a missing-price warning
      # that marks the report incomplete over a number nobody consumes.
      return 0.to_d if FIAT_CURRENCIES.include?(record.base_currency)

      return record.quote_amount.to_d if record.quote_currency == currency && record.quote_amount.present?

      if record.quote_currency.present? && STABLECOINS.include?(record.quote_currency) && record.quote_amount.present?
        return convert_fiat(amount: record.quote_amount.to_d, from: 'USD', to: currency, timestamp: record.transacted_at)
      end

      if record.quote_currency.present? && record.quote_amount.present? && FIAT_CURRENCIES.include?(record.quote_currency)
        return convert_fiat(amount: record.quote_amount.to_d, from: record.quote_currency, to: currency,
                            timestamp: record.transacted_at)
      end

      price = price_at(asset: record.base_currency, currency: currency, timestamp: record.transacted_at)
      price * record.base_amount.to_d
    end

    def resolve_fee_fiat_value(record, currency)
      return 0.to_d if record.fee_amount.blank? || record.fee_amount.zero?

      if record.fee_currency == currency
        record.fee_amount.to_d
      elsif FIAT_CURRENCIES.include?(record.fee_currency)
        convert_fiat(amount: record.fee_amount.to_d, from: record.fee_currency, to: currency,
                     timestamp: record.transacted_at)
      elsif STABLECOINS.include?(record.fee_currency)
        convert_fiat(amount: record.fee_amount.to_d, from: 'USD', to: currency, timestamp: record.transacted_at)
      else
        price = price_at(asset: record.fee_currency, currency: currency, timestamp: record.transacted_at)
        price * record.fee_amount.to_d
      end
    end

    # Kraken reports a trade as one ledger row per asset, with the fee on the row that paid it: a
    # EUR-funded BTC buy carries the whole fee on the EUR row, so the BTC lot would never see it,
    # and a BTC sale settled in EUR carries it on the EUR row the proceeds arrive on, so no engine
    # would deduct it from the gain. Move such a fee onto the crypto leg — in either direction —
    # when the pairing is unambiguous.
    def attribute_quote_row_fees(entries)
      entries.group_by { |entry| entry[:group_id] }.each do |group_id, group|
        next if group_id.blank? || group.size < 2

        donors = group.select { |entry| FIAT_CURRENCIES.include?(entry[:base_currency]) && entry[:fee_amount].present? }
        targets = group.select do |entry|
          TRADE_TYPES.include?(entry[:entry_type].to_s) && FIAT_CURRENCIES.exclude?(entry[:base_currency])
        end
        next unless donors.one? && targets.one?

        target = targets.first
        # A target with a fee of its own is a shape this model cannot merge (a quantity-shrinking
        # same-asset fee and a cost-adding fiat fee at once), so leave both rows alone.
        next if target[:fee_currency].present?

        target.merge!(donors.first.slice(:fee_currency, :fee_amount, :fee_fiat_value))
        donors.first.merge!(fee_currency: nil, fee_amount: nil, fee_fiat_value: 0.to_d)
      end
    end

    def transfer_fee_amount(transaction, links, deposit_amounts)
      deposit_id = links[transaction.id]
      return unless transaction.withdrawal? && deposit_id

      deposit_amount = deposit_amounts[deposit_id]
      return unless deposit_amount

      transaction.base_amount.to_d - deposit_amount.to_d
    end

    def fetch_single_price(asset:, currency:, timestamp:)
      coin_id = asset_to_coingecko_id(asset)
      return nil unless coin_id

      result = MarketData.get_historical_price_range(
        coin_id: coin_id,
        currency: currency.downcase,
        from: timestamp.beginning_of_day,
        to: timestamp.end_of_day
      )

      return nil if result.failure?

      prices = result.data['prices']
      return nil if prices.blank?

      price = prices.first[1].to_d
      # Same reason as `fetch_price_range`: `insert` skips the presence validation, so a zero stored
      # here would be read back as a valid price by every future report.
      return nil if price.zero?

      @price_cache["#{asset}/#{currency}/#{timestamp.to_date}"] = price
      HistoricalPrice.store(asset: asset, currency: currency, date: timestamp.to_date, price: price)
      price
    end

    def stablecoin_rate(currency, timestamp)
      return 1.to_d if currency == 'USD'

      fiat_exchange_rate(from: 'USD', to: currency, timestamp: timestamp)
    end

    def fiat_exchange_rate(from:, to:, timestamp:)
      fx_key = "FX/#{from}/#{to}/#{timestamp.to_date}"
      warning = "FX #{from}/#{to} #{timestamp.to_date}"
      # An approximated pair is cached like any other rate, but its warning has to fire again for
      # every record that leans on it: price_missing is a per-record warning delta, so a silent
      # cache hit would un-flag records 2..N of a report sharing one broken currency pair.
      @warnings << warning if @warned_fx_keys.include?(fx_key)
      cached = @price_cache[fx_key]
      return cached if cached

      @price_cache[fx_key] = Tax::EcbFxRates.rate(from: from, to: to, date: timestamp.to_date)
    rescue Tax::EcbFxRates::MissingRate
      @warned_fx_keys << fx_key
      @warnings << warning
      @price_cache[fx_key] = fallback_fx_rate(from: from, to: to, timestamp: timestamp)
    end

    def fallback_fx_rate(from:, to:, timestamp:)
      btc_cross_rate(from: from, to: to, timestamp: timestamp)
    rescue Tax::EcbFxRates::MissingRate
      0.to_d
    end

    # A BTC cross is only an approximation; its warning marks the affected record as incomplete.
    def btc_cross_rate(from:, to:, timestamp:)
      btc_from = price_at(asset: 'BTC', currency: from, timestamp: timestamp)
      btc_to = price_at(asset: 'BTC', currency: to, timestamp: timestamp)
      raise Tax::EcbFxRates::MissingRate, "#{from}/#{to} #{timestamp.to_date}" unless btc_from.positive? && btc_to.positive?

      btc_to / btc_from
    end

    def asset_to_coingecko_id(symbol)
      @asset_id_cache ||= {}
      @asset_id_cache[symbol] ||= Asset.find_by(symbol: symbol)&.external_id
    end
  end
end
