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
    # The two directions a grouped trade leg can take, whatever the venue calls them: Kraken books
    # every leg as `buy`/`sell`, Binance books every Convert and dust sweep as `swap_in`/`swap_out`.
    IN_LEGS = %w[buy swap_in].freeze
    OUT_LEGS = %w[sell swap_out].freeze
    private_constant :TRADE_TYPES, :IN_LEGS, :OUT_LEGS

    # The order every reader of the ledger walks it in. A swap's two legs share an instant, and the
    # venue decides which is stored first: the out-leg has to be seen first or its basis has nothing
    # to travel into. So a group sits where its first-stored leg was, out-legs ahead of in-legs
    # WITHIN it, and everything else keeps its stored order — an unrelated sale that happens to
    # share a second with a purchase must not jump ahead of it. A grouped `sell` counts as an
    # out-leg because a Kraken coin-for-coin trade is booked as one.
    def self.ordered(transactions)
      anchors = transactions.group_by { |tx| [tx.exchange_id, tx.group_id] }
                            .transform_values { |legs| legs.map(&:id).min }
      transactions.sort_by do |tx|
        anchor = tx.group_id.present? ? anchors[[tx.exchange_id, tx.group_id]] : tx.id
        [tx.transacted_at, anchor, tx.swap_out? || tx.sell? ? 0 : 1, tx.id]
      end
    end

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
        # A row the user has priced needs no price of ours, and must not be counted among the missing
        # ones — a report is not incomplete over a figure it was handed. Nor does a row the cash leg
        # beside it already values.
        next if cash_leg_value(tx)
        next if tx.respond_to?(:manual?) && tx.manual?(:fiat_value) && !tx.quoted?
        next if tx.quote_currency == currency && tx.quote_amount.present?
        next if tx.quote_currency.present? && tx.quote_amount.present? &&
                (STABLECOINS.include?(tx.quote_currency) || FIAT_CURRENCIES.include?(tx.quote_currency))

        add_coin_date(coins_needed, tx.base_currency, tx.transacted_at, tx.exchange)
        if tx.fee_amount.present? && tx.fee_amount.positive? && !STABLECOINS.include?(tx.fee_currency)
          add_coin_date(coins_needed, tx.fee_currency, tx.transacted_at, tx.exchange)
        end
      end

      if currency != 'USD'
        all_dates = coins_needed.values.flat_map { |info| info[:dates].to_a }
        add_coin_dates(coins_needed, 'BTC', all_dates) if all_dates.any?
      end

      # One call per COIN over its full range — a symbol that changed coin is two coins, each over
      # the days it was that coin.
      coins_needed.each_with_index do |((symbol, coin_id), info), index|
        Rails.logger.info("[TaxReport] Fetching prices for #{symbol} (#{index + 1}/#{coins_needed.size})")
        fetch_price_range(coin_id: coin_id, symbol: symbol, currency: currency,
                          from: info[:min], to: info[:max])
        on_progress&.call(index + 1, coins_needed.size)
      end
    end

    # `exchange` is where the row was booked: which coin a symbol means depends on the venue that
    # listed it and the day it was booked (`Tax::AssetIdentity`).
    def price_at(asset:, currency:, timestamp:, exchange: nil)
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
      price = fetch_single_price(asset: asset, currency: currency, timestamp: timestamp, exchange: exchange)
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
      # Before any price is looked up: a leg the cash beside it values is never fetched and never warns.
      @cash_legs = cash_leg_context(transactions, currency)
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
        fiat_value = resolve_row_value(tx, currency)
        fee_fiat_value = resolve_fee_fiat_value(tx, currency)
        leg = @cash_legs.fetch(tx.id, {})
        price_missing = @warnings.size > warnings_before || leg[:price_missing] == true

        enrich_percent = total.positive? ? 21 + ((index + 1).to_f / total * 79).to_i : 100
        on_progress&.call(enrich_percent, 100)

        {
          entry_type: leg[:entry_type] || tx.entry_type,
          base_currency: tx.base_currency,
          base_amount: tx.base_amount.to_d,
          quote_currency: leg[:quote_currency] || tx.quote_currency,
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
          # A figure the user stated by hand rather than one the venue reported or we priced. Carried
          # through so the tax report can disclose it: a stated cost is defensible, a silent one is not.
          stated_value: tx.respond_to?(:manual?) && tx.manual?(:fiat_value) && !valued_by_venue?(tx),
          exchange: tx.exchange.name_id,
          linked: links.key?(tx.id) || linked_deposit_ids.include?(tx.id),
          transfer_fee_amount: transfer_fee_amount(tx, links, deposit_amounts)
        }.merge(leg.slice(:swap_fiat_cost, :swap_stable_cost))
      end

      attribute_quote_row_fees(entries)
      entries
    end

    # One API call for a whole window, and only when the table does not already cover it. Public
    # because the portfolio backfill fetches on the same terms — one range per symbol, once.
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

    # A withdrawal's or a lost coin's own value is for the record — what left, and what it was worth
    # the day it left. No engine reads it (the FIFO family and PVCT branch on `linked` and
    # `transfer_fee_amount`, the wealth snapshot never sees enriched rows), so a chart with no price
    # for it is not a hole in the report: the base lookup never warns. Its fee does reach the
    # figures, and warns as on any row.
    def resolve_row_value(transaction, currency)
      return resolve_fiat_value(transaction, currency) unless transaction.withdrawal? || transaction.lost?

      kept = @warnings.size
      value = resolve_fiat_value(transaction, currency)
      @warnings.slice!(kept..)
      value
    end

    def cash_leg_value(transaction)
      @cash_legs&.dig(transaction.id, :fiat_value)
    end

    # By a quote of its own or by the cash leg beside it — without a query per row: the group
    # context is already built.
    def valued_by_venue?(transaction)
      (transaction.respond_to?(:quoted?) && transaction.quoted?) || cash_leg_value(transaction).present?
    end

    # Where a legless row gets its value: the cash leg opposite it in its group. Kraken books every
    # trade as one row per asset with no quote, Binance books every Convert and dust sweep as swap
    # legs with no quote — and the cash row beside the coin row says exactly what was paid or
    # received, where a chart can only guess. Consulted ahead of any price lookup, behind a stated
    # value and a quote of the row's own. Groups are a venue's own; a blank id is never a group.
    #
    # The cash is split fiat from stablecoin so the engine can take the stablecoin half only where
    # a stablecoin is cash (`stablecoin_as_fiat`), and it is the cash row's amount alone — its fee
    # is moved onto the coin leg by `attribute_quote_row_fees`, never counted twice.
    def cash_leg_context(transactions, currency)
      transactions.group_by { |tx| [tx.exchange_id, tx.group_id] }.each_with_object({}) do |((_, group_id), legs), context|
        next if group_id.blank? || legs.size < 2

        annotate_group(context, legs, currency)
      end
    end

    def annotate_group(context, legs, currency)
      cash, coins = legs.partition { |tx| cash?(tx.base_currency) }
      outs = coins.select { |tx| OUT_LEGS.include?(tx.entry_type) && tx.quote_amount.blank? }
      ins = coins.select { |tx| IN_LEGS.include?(tx.entry_type) && tx.quote_amount.blank? }
      cash_out = cash.select { |tx| OUT_LEGS.include?(tx.entry_type) }
      cash_in = cash.select { |tx| IN_LEGS.include?(tx.entry_type) }

      if cash.empty?
        # A coin traded for a coin is a swap, however the venue books it — so it chains where a
        # swap chains and realises where one realises, on every venue alike.
        return unless outs.any? && ins.any?

        outs.each { |tx| context[tx.id] = { entry_type: 'swap_out' } }
        ins.each { |tx| context[tx.id] = { entry_type: 'swap_in' } }
      elsif cash_in.any? && ins.any?
        # Cash AND coins received for the same coins is a shape no importer produces. Refused, not
        # half-chained: the out-legs sell at market, the in-legs open at market, all marked.
        outs.each { |tx| context[tx.id] = { price_missing: true, quote_currency: quote_of(cash_in) } }
        ins.each { |tx| context[tx.id] = { price_missing: true } }
      else
        value_in_legs(context, ins, cash_out, mixed: outs.any?, currency: currency) if cash_out.any? && ins.any?
        value_out_legs(context, outs, cash_in, currency: currency) if cash_in.any? && outs.any?
      end
    end

    def value_in_legs(context, ins, cash_out, mixed:, currency:)
      fiat, stable = cash_faces(cash_out, currency)
      return unless fiat

      shares(ins).each do |tx, (share, assumed)|
        leg = { swap_fiat_cost: fiat * share, swap_stable_cost: stable * share }
        # A group whose out-legs are all cash states the face — that IS the market, exactly. A mixed
        # sweep (coins and cash into BNB) keeps the recipient's market value, which the taxable
        # engines open the lot at, and carries the cash beside it for the chain to add.
        leg[:fiat_value] = (fiat + stable) * share unless mixed
        leg[:price_missing] = true if assumed
        context[tx.id] = leg
      end
    end

    def value_out_legs(context, outs, cash_in, currency:)
      fiat, stable = cash_faces(cash_in, currency)
      return unless fiat

      quote = quote_of(cash_in)
      shares(outs).each do |tx, (share, assumed)|
        context[tx.id] = { fiat_value: (fiat + stable) * share, quote_currency: quote, price_missing: assumed }
      end
    end

    # A fiat currency if any leg carries one — `fiat_disposal?` then holds under every flag.
    def quote_of(cash_legs)
      (cash_legs.find { |tx| FIAT_CURRENCIES.include?(tx.base_currency) } || cash_legs.first).base_currency
    end

    # By amount while the legs are one asset; anything else has no scale to share on yet (no price
    # has been looked up), so it is split evenly and every leg says so.
    def shares(legs)
      one_asset = legs.map(&:base_currency).uniq.one?
      total = legs.sum(0.to_d) { |tx| tx.base_amount.to_d }
      legs.index_with do |tx|
        if one_asset && total.positive?
          [tx.base_amount.to_d / total, false]
        else
          [1.to_d / legs.size, true]
        end
      end
    end

    # [fiat, stablecoin] faces in the report currency — or nil when a leg could not be converted, in
    # which case the coin legs are priced as any other row would be, and warn on their own terms: a
    # chart price is still a valuation, a cash figure in a currency with no rate is not.
    def cash_faces(cash_legs, currency)
      kept = @warnings.size
      fiat = 0.to_d
      stable = 0.to_d
      cash_legs.each do |tx|
        amount = tx.base_amount.to_d
        if STABLECOINS.include?(tx.base_currency)
          stable += convert_fiat(amount: amount, from: 'USD', to: currency, timestamp: tx.transacted_at)
        else
          fiat += convert_fiat(amount: amount, from: tx.base_currency, to: currency, timestamp: tx.transacted_at)
        end
      end
      return [fiat, stable] if @warnings.size == kept

      @warnings.slice!(kept..)
      nil
    end

    def cash?(currency)
      FIAT_CURRENCIES.include?(currency) || STABLECOINS.include?(currency)
    end

    # Keyed by symbol AND coin: a symbol that changed coin (`Tax::AssetIdentity`) is fetched as each
    # coin over its own days. A symbol nobody can name a coin for is not fetched at all.
    def add_coin_date(coins, symbol, timestamp, exchange = nil)
      return if symbol.blank? || FIAT_CURRENCIES.include?(symbol)

      coin_id = coin_id_for(symbol, exchange, timestamp)
      return unless coin_id

      date = timestamp.to_date
      key = [symbol, coin_id]
      coins[key] ||= { min: date, max: date, dates: Set.new }
      coins[key][:min] = date if date < coins[key][:min]
      coins[key][:max] = date if date > coins[key][:max]
      coins[key][:dates] << date
    end

    def add_coin_dates(coins, symbol, dates)
      dates.each do |d|
        date = d.is_a?(Date) ? d : d.to_date
        add_coin_date(coins, symbol, date.to_datetime)
      end
    end

    # The alias is a date's question and costs no query; the catalogue answer is cached per venue.
    def coin_id_for(symbol, exchange, timestamp)
      exchange = venue(exchange)
      Tax::AssetIdentity.alias_coin(symbol, exchange: exchange, at: timestamp) ||
        (@coin_ids ||= {})[[symbol, exchange&.id]] ||= Tax::AssetIdentity.catalogue_coin(symbol, exchange: exchange)
    end

    # A venue handed over as a record, or named — an enriched row carries only its `name_id`, and
    # the engines that value a holding at a date have nothing else to say where it was booked.
    def venue(exchange)
      return exchange unless exchange.is_a?(String) || exchange.is_a?(Symbol)

      (@venues ||= {})[exchange.to_s] ||= Exchange.find_by(type: "Exchanges::#{exchange.to_s.camelize}")
    end

    def resolve_fiat_value(record, currency)
      # `prefetch`/`add_coin_date` already refuses to price a fiat base. Report drops fiat rows before
      # the engines, so nothing reads this value; pricing it can only fabricate a missing-price warning
      # that marks the report incomplete over a number nobody consumes.
      return 0.to_d if FIAT_CURRENCIES.include?(record.base_currency)

      # The venue's own figure first: amount and price of the row's own are the value, and nothing
      # stands in front of them.
      return record.quote_amount.to_d if record.quote_currency == currency && record.quote_amount.present?

      if record.quote_currency.present? && STABLECOINS.include?(record.quote_currency) && record.quote_amount.present?
        return convert_fiat(amount: record.quote_amount.to_d, from: 'USD', to: currency, timestamp: record.transacted_at)
      end

      if record.quote_currency.present? && record.quote_amount.present? && FIAT_CURRENCIES.include?(record.quote_currency)
        return convert_fiat(amount: record.quote_amount.to_d, from: record.quote_currency, to: currency,
                            timestamp: record.transacted_at)
      end

      # The cash leg beside it is the venue's figure too: what the coins fetched, or cost.
      cash = cash_leg_value(record)
      return cash if cash

      # Then what the USER says it was worth, ahead of everything the app would have to guess. This
      # is the single point every consumer of a priced row passes through, so the tiles, the chart,
      # the positions and the tax report inherit a stated value without knowing it exists. Stated
      # in USD, like everything else behind the page.
      stated = record.respond_to?(:manual_value) && record.manual_value(:fiat_value)
      return convert_fiat(amount: stated, from: 'USD', to: currency, timestamp: record.transacted_at) if stated

      price = price_at(asset: record.base_currency, currency: currency, timestamp: record.transacted_at,
                       exchange: record.exchange)
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
        price = price_at(asset: record.fee_currency, currency: currency, timestamp: record.transacted_at,
                         exchange: record.exchange)
        price * record.fee_amount.to_d
      end
    end

    # Kraken reports a trade as one ledger row per asset, with the fee on the row that paid it: a
    # EUR-funded BTC buy carries the whole fee on the EUR row, so the BTC lot would never see it,
    # and a BTC sale settled in EUR carries it on the EUR row the proceeds arrive on, so no engine
    # would deduct it from the gain. Move such a fee onto the crypto leg — in either direction —
    # when the pairing is unambiguous.
    def attribute_quote_row_fees(entries)
      entries.group_by { |entry| [entry[:exchange], entry[:group_id]] }.each do |(_, group_id), group|
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
        # A fee that could not be converted is incomplete wherever it lands.
        target[:price_missing] ||= donors.first[:price_missing]
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

    # Half the window either side of the day in question. The data API serves its price ARCHIVE only
    # for spans WIDER than 90 days: at or below that, CoinGecko returns sub-daily density the archive
    # cannot reproduce, so it steps aside for the live proxy — whose plan then refuses anything older
    # than two years. A one-day question fell straight between the two and came back empty for every
    # date before that window, though the archive held the price the whole time. Verified against
    # production: the same start date returns nothing as one day and $56,020 as part of a six-month
    # span. So this window is not a tuning knob — below 121 days the archive goes back out of reach.
    SINGLE_PRICE_WINDOW = 60.days

    # One day's price, asked for as a window the archive will answer. The days either side are real
    # prices that were fetched anyway, so `fetch_price_range` stores them on the way past and the
    # next row on a neighbouring date costs nothing — which is also why this delegates rather than
    # keeping a second, subtly different fetch-and-store of its own.
    def fetch_single_price(asset:, currency:, timestamp:, exchange: nil)
      date = timestamp.to_date
      # Anchored on the END, so a recent date whose window would run past today is widened backwards
      # instead of being narrowed under the threshold. Tomorrow has no price and never will.
      to = [date + SINGLE_PRICE_WINDOW, Date.current].min
      from = to - (SINGLE_PRICE_WINDOW * 2)
      # And never across the day a symbol changed coin: the window is clipped to the coin's own
      # days, or Terra Classic's prices would be stored as LUNA's for days after the relaunch, and
      # storage being insert-only, the right coin could never replace them. A window cut short by
      # the boundary behind it runs forward instead, so the archive still sees a span it answers.
      range, coin_id = Tax::AssetIdentity.coin_ids_over(asset, exchange: venue(exchange), from: from, to: to)
                                         .find { |days, _| days.cover?(date) }
      return nil unless coin_id

      last = range.end == to ? [range.begin + (SINGLE_PRICE_WINDOW * 2), Date.current].min : range.end
      fetch_price_range(coin_id: coin_id, symbol: asset, currency: currency, from: range.begin, to: last)
      @price_cache["#{asset}/#{currency}/#{date}"]&.to_d&.nonzero?
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
  end
end
