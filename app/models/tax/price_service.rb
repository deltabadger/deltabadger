module Tax
  class PriceService
    STABLECOINS = %w[USDT USDC BUSD DAI FDUSD TUSD PYUSD RLUSD].freeze
    FIAT_CURRENCIES = %w[USD EUR GBP CHF SEK PLN DKK CZK BGN].freeze

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
      return @price_cache[cache_key] if @price_cache[cache_key]

      # Check DB
      db_price = HistoricalPrice.lookup(asset: asset, currency: currency, date: timestamp.to_date)
      if db_price
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
      transactions.each_with_index.map do |tx, index|
        warnings_before = @warnings.size
        fiat_value = resolve_fiat_value(tx, currency)
        fee_fiat_value = resolve_fee_fiat_value(tx, currency)
        price_missing = @warnings.size > warnings_before

        enrich_percent = total.positive? ? 21 + ((index + 1).to_f / total * 79).to_i : 100
        on_progress&.call(enrich_percent, 100)

        {
          entry_type: tx.entry_type,
          base_currency: tx.base_currency,
          base_amount: tx.base_amount.to_d,
          quote_currency: tx.quote_currency,
          quote_amount: tx.quote_amount&.to_d,
          fiat_value: fiat_value,
          fee_fiat_value: fee_fiat_value,
          fee_currency: tx.fee_currency,
          transacted_at: tx.transacted_at,
          tx_id: tx.tx_id,
          group_id: tx.group_id,
          price_missing: price_missing,
          exchange: tx.exchange.name_id,
          linked: links.key?(tx.id) || linked_deposit_ids.include?(tx.id),
          transfer_fee_amount: transfer_fee_amount(tx, links, deposit_amounts)
        }
      end
    end

    private

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
        date = Time.at(timestamp_ms / 1000.0).utc.to_date
        cache_key = "#{symbol}/#{currency}/#{date}"
        next if @price_cache[cache_key] # already from DB

        @price_cache[cache_key] = price.to_d
        records_to_store << { asset: symbol, currency: currency, date: date, price: price.to_d }
      end

      HistoricalPrice.bulk_store(records_to_store)
    end

    def resolve_fiat_value(record, currency)
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
      elsif STABLECOINS.include?(record.fee_currency)
        convert_fiat(amount: record.fee_amount.to_d, from: 'USD', to: currency, timestamp: record.transacted_at)
      else
        price = price_at(asset: record.fee_currency, currency: currency, timestamp: record.transacted_at)
        price * record.fee_amount.to_d
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
