module Tax
  # Figures for the German Anlage KAP / KAP-INV from a broker account, as a pure calculation
  # returning a structured result; rendering and delivery live elsewhere.
  #
  # This is a form a taxpayer signs, so the design is refusal-first: a symbol whose history is
  # incomplete (missing acquisitions, an unpriceable leg, an unmodelled corporate action, a
  # pre-2018 fund lot) contributes NOTHING to the summaries and says so, rather than contributing a
  # figure that merely looks plausible. Refusal is per symbol; only an unclassified instrument, where
  # the whole bucketing is unknown, blocks every summary.
  #
  # It deliberately does not reuse the crypto pipeline: `Tax::Methods::*` aggregate away the matched
  # lots, year-boundary holdings and per-lot Vorabpauschale this form needs, and
  # `Tax::Report#taxable_entries` drops every fiat-base row — which is exactly where a broker's
  # dividends, interest and withholding live. Values are unrounded BigDecimal end to end; the
  # renderer rounds once.
  class BrokerReport
    # The annex verified the form line numbers against Germany's Anlage KAP and KAP-INV.
    COUNTRY = 'DE'.freeze
    # The annex verified 2023–2025; extend this range when a new year's form is verified.
    SUPPORTED_YEARS = (2023..2025)

    REFUSAL_REASON_ORDER = %i[
      uncovered_disposal unsupported_activity pre_2018_fund_lot missing_price missing_fx missing_basiszins
    ].freeze
    private_constant :REFUSAL_REASON_ORDER

    def initialize(user:, year:, exchange:, api_key: nil)
      @user = user
      @year = year
      @exchange = exchange
      @api_key = api_key || exchange.api_keys.where(user: user).correct.first || exchange.api_keys.find_by(user: user)
    end

    def result
      @result ||= calculate
    end

    # Zeile labels transcribe a German form the user is copying into, so they follow that form's
    # language rather than the UI's, as Tax::Report#to_csv already does via its jurisdiction locale.
    def to_csv
      I18n.with_locale(Tax::Jurisdictions.for(COUNTRY)[:locale]) do
        Tax::BrokerReportCsv.new(result).to_csv
      end
    end

    # The classification panel's to-do list. Same universe, same symbol resolver and the same
    # FundClassification proposals a full run uses — a second query would disagree with the
    # report's own refusal. Deliberately stops before the walk: nothing here reads an FX rate
    # or a price, so it is cheap enough to render a modal with.
    def classification_rows
      initialize_calculation
      @records = universe.reject { |record| cryptocurrency?(instrument_symbol(record)) }
      build_symbol_states
      # The two refusals the panel can afford: `prepare_refusals` reads only the records already in
      # memory. The other four are raised by the walk, which prices and converts, and stay a matter
      # for the report's own warnings.
      prepare_refusals
      symbol_rows
    end

    private

    def calculate
      Tax::EcbFxRates.ensure_loaded!
      initialize_calculation
      @records = universe.reject { |record| cryptocurrency?(instrument_symbol(record)) }
      build_symbol_states
      prepare_distribution_gross_up
      prepare_refusals
      prepare_fx_rates
      @states.each_value { |state| walk_symbol(state) }
      walk_cash_records
      finalize_worksheets
      build_result
    end

    def initialize_calculation
      @record_symbols = {}
      @record_fx_rates = {}
      @fx_rate_cache = {}
      @boundary_price_cache = {}
      @refusals = Hash.new { |hash, symbol| hash[symbol] = Set.new }
      @warnings = []
      @warning_keys = Set.new
      @disposals = []
      @income = []
      @fee_total_eur = 0.to_d
      @cash_kap = empty_kap
    end

    def universe
      AccountTransaction.for_user(@user).for_exchange(@exchange)
                        .where(transacted_at: ...Time.utc(@year + 1))
                        .order(transacted_at: :asc).to_a
    end

    # The single symbol resolver: universe filtering, classification, warnings and the UI's to-do
    # list all go through it, so a security is spelled one way everywhere. The dividend family and
    # withholding carry the security in `quote_currency` with USD in `base_currency`, so reading
    # `base_currency` raw would file every dividend under "USD". A fiat code resolving to nil is what
    # makes a symbol-less unsupported activity a cash-only unknown rather than an instrument the user
    # is asked to classify.
    def instrument_symbol(record)
      @record_symbols.fetch(record.id) do
        raw = record.other_income? || record.withholding_tax? ? record.quote_currency : record.base_currency
        symbol = raw unless raw.blank? || Tax::PriceService::FIAT_CURRENCIES.include?(raw)
        @record_symbols[record.id] = symbol
      end
    end

    def cryptocurrency?(symbol)
      crypto_scope.crypto?(symbol) do |categories|
        add_warning(code: :ambiguous_symbol, symbol: symbol, detail: categories)
      end
    end

    def crypto_scope
      @crypto_scope ||= Tax::CryptoScope.new(user: @user)
    end

    def assets_for(symbol)
      crypto_scope.assets_for(symbol)
    end

    # The stock/ETF row is the one that carries an `instrument_type` for FundClassification to
    # propose from, so a collision must not hand the resolver a coin instead.
    def asset_for(symbol)
      assets = assets_for(symbol)
      assets.find { |asset| asset.instrument_type.in?(%w[stock etf]) } || assets.first
    end

    def build_symbol_states
      symbols = @records.filter_map { |record| instrument_symbol(record) }.uniq.sort
      @states = symbols.index_with do |symbol|
        classification = FundClassification.resolve(user: @user, symbol: symbol, asset: asset_for(symbol))
        {
          symbol: symbol,
          classification: classification,
          kind: classification.kind&.to_sym,
          fund_category: classification.fund_category&.to_sym,
          records: @records.select { |record| instrument_symbol(record) == symbol },
          ledger: Tax::LotLedger.new,
          distributions: [],
          kap: empty_kap,
          kap_inv: empty_kap_inv
        }
      end
    end

    def prepare_distribution_gross_up
      withholding = @records.select(&:withholding_tax?).group_by do |record|
        next unless instrument_symbol(record) && activity_type(record) == 'DIVFT'

        [instrument_symbol(record), record_date(record)]
      end
      divft_by_key = withholding.each_with_object({}) do |(key, records), totals|
        next unless key

        totals[key] = records.sum(0.to_d) { |record| record.base_amount.to_d.abs }
      end

      distributions = @records.select { |record| record.other_income? && instrument_symbol(record) }
      @distribution_gross_usd = {}
      @distribution_scale = {}
      distributions.group_by { |record| [instrument_symbol(record), record_date(record)] }.each do |key, records|
        net_total = records.sum(0.to_d) { |record| signed_usd(record) }
        divft_total = divft_by_key.fetch(key, 0.to_d)
        scale = net_total.zero? ? 1.to_d : (net_total + divft_total) / net_total
        gross_remaining = net_total + divft_total

        records.each_with_index do |record, index|
          gross = index == records.length - 1 ? gross_remaining : signed_usd(record) * scale
          @distribution_gross_usd[record.id] = gross
          @distribution_scale[record.id] = scale
          gross_remaining -= gross
        end
      end
    end

    def prepare_refusals
      @records.select(&:unsupported_activity?).each do |record|
        symbol = instrument_symbol(record)
        detail = activity_type(record)
        if symbol
          refuse_symbol(symbol, :unsupported_activity, detail: detail)
        else
          add_warning(code: :unsupported_activity, symbol: nil, detail: detail)
        end
      end

      @states.each_value do |state|
        next unless state[:kind] == :fund

        state[:records].select(&:buy?).each do |record|
          acquired_on = record_date(record)
          next unless acquired_on < Date.new(2018, 1, 1)

          refuse_symbol(state[:symbol], :pre_2018_fund_lot, detail: acquired_on)
        end
      end
    end

    def prepare_fx_rates
      @records.each do |record|
        next unless fx_required?(record)

        rate = usd_per_eur(record_date(record))
        @record_fx_rates[record.id] = rate
        next if rate

        symbol = instrument_symbol(record)
        if symbol
          refuse_symbol(symbol, :missing_fx, detail: record_date(record))
        else
          add_warning(code: :missing_fx, symbol: nil, detail: record_date(record))
        end
      end
    end

    def fx_required?(record)
      record.buy? || record.sell? || record.other_income? || record.withholding_tax? ||
        record.return_of_capital? || record.fee?
    end

    def usd_per_eur(date)
      return @fx_rate_cache[date] if @fx_rate_cache.key?(date)

      @fx_rate_cache[date] = Tax::EcbFxRates.rate(from: 'EUR', to: 'USD', date: date)
    rescue Tax::EcbFxRates::MissingRate
      @fx_rate_cache[date] = nil
    end

    def usd_to_eur(amount, rate)
      return 0.to_d if amount.nil?
      return unless rate

      # Each statutory USD leg uses its own date's published USD-per-EUR quote. Dividing by the
      # exact quote avoids the last-digit loss caused by multiplying with a rounded reciprocal.
      amount.to_d / rate
    end

    def walk_symbol(state)
      first_acquisition = state[:records].select(&:buy?).map { |record| record_date(record) }.min
      boundary_year = first_acquisition&.year

      state[:records].each do |record|
        boundary_year = accrue_boundaries_before(state, boundary_year, record.transacted_at)
        handle_symbol_record(state, record)
      end
      accrue_remaining_boundaries(state, boundary_year)
    end

    def accrue_boundaries_before(state, boundary_year, timestamp)
      while boundary_year && boundary_year <= @year - 1 && timestamp >= Time.utc(boundary_year + 1)
        accrue_vorabpauschale(state: state, computation_year: boundary_year) unless refused?(state[:symbol])
        boundary_year += 1
      end
      boundary_year
    end

    def accrue_remaining_boundaries(state, boundary_year)
      while boundary_year && boundary_year <= @year - 1
        accrue_vorabpauschale(state: state, computation_year: boundary_year) unless refused?(state[:symbol])
        boundary_year += 1
      end
    end

    def handle_symbol_record(state, record)
      case record.entry_type
      when 'buy'
        handle_buy(state, record)
      when 'sell'
        handle_sell(state, record)
      when 'adjustment'
        # Alpaca has already merged split legs, so base_amount is the only trustworthy net delta.
        state[:ledger].adjust(delta: record.base_amount.to_d, date: record_date(record))
      when 'return_of_capital'
        handle_return_of_capital(state, record)
      when 'other_income'
        handle_distribution(state, record)
      when 'withholding_tax'
        handle_withholding(state, record)
      when 'unsupported_activity'
        # Pricing an inert option/merger row can only add misleading missing-price noise. Its
        # preflight refusal is stronger and deliberately leaves the ledger untouched.
        nil
      end
    end

    def handle_buy(state, record)
      rate = @record_fx_rates[record.id]
      quote_usd = (record.quote_amount || 0).to_d
      fee_usd = (record.fee_amount || 0).to_d
      # A fill and its direct fee share one execution-date rate, so dividing their combined USD
      # cost once avoids creating a false last-digit residue by adding two recurring quotients.
      cost_eur = usd_to_eur(quote_usd + fee_usd, rate)
      state[:ledger].acquire(
        units: record.base_amount.to_d,
        cost_eur: cost_eur || 0.to_d,
        date: record_date(record),
        meta: { fx_rate: rate }
      )
    end

    def handle_sell(state, record)
      sold_on = record_date(record)
      matches = state[:ledger].dispose(units: record.base_amount.to_d, date: sold_on)
      refuse_symbol(state[:symbol], :uncovered_disposal, detail: sold_on) if matches.any? { |match| match[:uncovered] }

      rate = @record_fx_rates[record.id]
      quote_usd = (record.quote_amount || 0).to_d
      fee_usd = (record.fee_amount || 0).to_d
      fee_eur = usd_to_eur(record.fee_amount, rate)
      proceeds_eur = usd_to_eur(quote_usd - fee_usd, rate)
      cost_eur = matches.sum(0.to_d) { |match| match[:cost_eur] }
      vap_eur = matches.sum(0.to_d) { |match| match[:vap_eur] }
      gain_eur = disposal_gain(state, proceeds_eur, cost_eur, vap_eur)

      add_disposal_contribution(state, gain_eur) if report_year?(record) && gain_eur
      return unless report_year?(record)

      @disposals << {
        symbol: state[:symbol],
        kind: state[:kind],
        fund_category: state[:fund_category],
        sold_on: sold_on,
        units: record.base_amount.to_d,
        proceeds_eur: proceeds_eur,
        proceeds_usd: quote_usd - fee_usd,
        fx_rate: rate,
        fee_eur: fee_eur,
        cost_eur: cost_eur,
        gain_eur: gain_eur,
        vorabpauschale_deducted_eur: vap_eur,
        refused: false,
        matches: disposal_matches(matches)
      }
    end

    def disposal_gain(state, proceeds_eur, cost_eur, vap_eur)
      return unless proceeds_eur

      gain = proceeds_eur - cost_eur
      state[:kind] == :fund ? gain - vap_eur : gain
    end

    def disposal_matches(matches)
      matches.map do |match|
        {
          acquired_on: match[:acquired_on],
          units: match[:units_taken],
          cost_eur: match[:cost_eur],
          fx_rate: match[:lot]&.meta&.[](:fx_rate),
          vap_eur: match[:vap_eur],
          uncovered: match[:uncovered] || false
        }
      end
    end

    def add_disposal_contribution(state, gain)
      case state[:kind]
      when :fund
        state[:kap_inv][:sale_result] += gain
      when :share
        state[:kap][:z19_saldo] += gain
        state[:kap][:z20_share_gains] += gain if gain.positive?
        # German KAP loss lines take positive magnitudes even though Z19 remains a signed saldo.
        state[:kap][:z23_share_losses] += -gain if gain.negative?
      when :other_security
        state[:kap][:z19_saldo] += gain
        # Z22 follows the form convention while its contained-in-Z19 contribution stays negative.
        state[:kap][:z22_other_losses] += -gain if gain.negative?
      end
    end

    def handle_distribution(state, record)
      rate = @record_fx_rates[record.id]
      gross_usd = @distribution_gross_usd.fetch(record.id) { signed_usd(record) }
      gross_eur = usd_to_eur(gross_usd, rate)
      scale = @distribution_scale.fetch(record.id, 1.to_d)
      raw_per_share = raw_data(record)['per_share_amount']
      per_share_usd = raw_per_share.present? ? raw_per_share.to_d * scale : nil
      date = record_date(record)
      # The explicit acquisition filter makes retrospective distribution ratios safe. At walk
      # time current lot units are already expressed in the distribution date's share terms.
      units_outstanding = state[:ledger].open_lots.select { |lot| lot.acquired_on <= date }
                                        .sum(0.to_d, &:units)
      state[:distributions] << {
        date: date,
        gross_usd: gross_usd,
        gross_eur: gross_eur,
        per_share_usd: per_share_usd,
        units_outstanding: units_outstanding,
        fx_rate: rate
      }
      return unless report_year?(record)

      bucket = state[:kind] == :fund ? :fund_distribution : :kap_dividend
      add_income_row(state: state, record: record, bucket: bucket, usd_amount: gross_usd,
                     eur_amount: gross_eur, fx_rate: rate)
      return unless gross_eur

      if state[:kind] == :fund
        state[:kap_inv][:distributions] += gross_eur
      elsif state[:kind]
        state[:kap][:z19_saldo] += gross_eur
      end
    end

    def handle_withholding(state, record)
      rate = @record_fx_rates[record.id]
      usd_amount = record.base_amount.to_d.abs
      eur_amount = usd_to_eur(usd_amount, rate)
      return unless report_year?(record)

      add_income_row(state: state, record: record, bucket: :withholding_tax, usd_amount: usd_amount,
                     eur_amount: eur_amount, fx_rate: rate)
      state[:kap][:z41_withholding_tax] += eur_amount if eur_amount
    end

    def handle_return_of_capital(state, record)
      rate = @record_fx_rates[record.id]
      raw_per_share = raw_data(record)['per_share_amount']
      per_share = raw_per_share&.to_d
      excess = if per_share&.positive?
                 reduction = usd_to_eur(per_share, rate)
                 state[:ledger].reduce_basis(per_unit_eur: reduction) if reduction
               else
                 reduction = usd_to_eur(record.quote_amount&.abs, rate)
                 state[:ledger].reduce_basis_total(amount_eur: reduction) if reduction
               end
      return unless report_year?(record)

      # The excess is a EUR residue of a basis reduction, not a broker-stated amount. Back-converting
      # it would put a derived number in a column that is a source figure on every other income row.
      add_income_row(state: state, record: record, bucket: :return_of_capital_excess,
                     usd_amount: nil, eur_amount: excess, fx_rate: rate)
      return unless excess&.positive?

      if state[:kind] == :fund
        state[:kap_inv][:distributions] += excess
      elsif state[:kind]
        state[:kap][:z19_saldo] += excess
      end
    end

    def walk_cash_records
      @records.each do |record|
        next if instrument_symbol(record)

        case record.entry_type
        when 'other_income'
          handle_interest(record)
        when 'withholding_tax'
          handle_cash_withholding(record)
        when 'fee'
          handle_fee(record)
        when 'unsupported_activity'
          # Cash-only unknown activity was warned in preflight and never reaches FX or pricing.
          nil
        end
      end
    end

    def handle_interest(record)
      rate = @record_fx_rates[record.id]
      usd_amount = signed_usd(record)
      eur_amount = usd_to_eur(usd_amount, rate)
      return unless report_year?(record)

      add_cash_income_row(record: record, bucket: :kap_interest, usd_amount: usd_amount,
                          eur_amount: eur_amount, fx_rate: rate)
      @cash_kap[:z19_saldo] += eur_amount if eur_amount
    end

    def handle_cash_withholding(record)
      rate = @record_fx_rates[record.id]
      usd_amount = record.base_amount.to_d.abs
      eur_amount = usd_to_eur(usd_amount, rate)
      return unless report_year?(record)

      add_cash_income_row(record: record, bucket: :withholding_tax, usd_amount: usd_amount,
                          eur_amount: eur_amount, fx_rate: rate)
      @cash_kap[:z41_withholding_tax] += eur_amount if eur_amount
    end

    def handle_fee(record)
      return unless report_year?(record)

      eur_amount = usd_to_eur(record.base_amount.to_d.abs, @record_fx_rates[record.id])
      return unless eur_amount

      # Alpaca's daily regulatory aggregate cannot be attributed to a disposal, so deducting it
      # under §20(4) would overstate sale costs. It is disclosed but excluded from every bucket.
      @fee_total_eur += eur_amount
    end

    def add_income_row(state:, record:, bucket:, usd_amount:, eur_amount:, fx_rate:)
      @income << {
        symbol: state[:symbol],
        date: record_date(record),
        activity_type: activity_type(record),
        bucket: bucket,
        usd_amount: usd_amount,
        fx_rate: fx_rate,
        eur_amount: eur_amount,
        refused: false
      }
    end

    def add_cash_income_row(record:, bucket:, usd_amount:, eur_amount:, fx_rate:)
      @income << {
        symbol: nil,
        date: record_date(record),
        activity_type: activity_type(record),
        bucket: bucket,
        usd_amount: usd_amount,
        fx_rate: fx_rate,
        eur_amount: eur_amount,
        refused: false
      }
    end

    def accrue_vorabpauschale(state:, computation_year:)
      return unless state[:kind] == :fund
      return if state[:ledger].open_lots.empty?

      symbol = state[:symbol]
      start_price_eur = price_eur(symbol, computation_year - 1)
      end_price_eur = price_eur(symbol, computation_year)
      unless start_price_eur && end_price_eur
        refuse_symbol(symbol, :missing_price, detail: computation_year)
        return
      end

      year_start = Date.new(computation_year, 1, 1)
      year_end = Date.new(computation_year, 12, 31)
      accruals = state[:ledger].open_lots.filter_map do |lot|
        # Vorabpauschale treats an out-of-year acquisition as a full-year lot, so this explicit
        # guard prevents a future lot from fabricating positive income at an earlier boundary.
        next if lot.acquired_on > year_end

        # A mid-year lot deliberately uses year-start share terms without an acquisition-date
        # filter. Its §18(2) month fraction, not a false zero start value, captures the short year.
        start_value_eur = lot.units_in_terms_of(year_start) * start_price_eur
        end_value_eur = lot.units_in_terms_of(year_end) * end_price_eur
        distributions_eur = lot_distributions_eur(state, lot, computation_year)
        vap = begin
          Tax::Vorabpauschale.for_lot(
            computation_year: computation_year,
            acquired_on: lot.acquired_on,
            start_value_eur: start_value_eur,
            end_value_eur: end_value_eur,
            distributions_eur: distributions_eur
          )
        rescue KeyError
          # Only the Basiszins lookup raises this. Scoped to the one call so an unrelated KeyError
          # elsewhere in the block can never be reported to the taxpayer as an unsupported year.
          break :missing_basiszins
        end
        [lot, vap]
      end

      if accruals == :missing_basiszins
        refuse_symbol(symbol, :missing_basiszins, detail: computation_year)
        return
      end

      accruals.each do |lot, vap|
        # Per-unit storage lets LotLedger rescale accrued VAP through every later split without
        # changing the lot's total §19 deduction.
        lot.meta[:vap_per_unit] += vap / lot.units if lot.units.positive?
        # §18(3) declares computation year N-1 in tax year N. Earlier years stay only in the lot
        # so a later disposal can deduct their accumulated gross VAP under §19.
        state[:kap_inv][:vorabpauschale] += vap if computation_year == @year - 1
      end
    end

    def price_eur(symbol, calendar_year)
      key = [symbol, calendar_year]
      return @boundary_price_cache[key] if @boundary_price_cache.key?(key)

      year_end = Date.new(calendar_year, 12, 31)
      prices = price_service.stock_price_range(
        exchange: @exchange,
        api_key: @api_key,
        symbol: symbol,
        from: year_end - 9,
        to: year_end
      )
      @boundary_price_cache[key] = prices.max_by { |date, _price| date }&.last
    end

    def price_service
      @price_service ||= Tax::PriceService.new
    end

    def lot_distributions_eur(state, lot, computation_year)
      state[:distributions].sum(0.to_d) do |distribution|
        next 0.to_d unless distribution[:date].year == computation_year
        next 0.to_d if distribution[:date] < lot.acquired_on

        distribution_eur_for_lot(distribution, lot)
      end
    end

    def distribution_eur_for_lot(distribution, lot)
      units = lot.units_in_terms_of(distribution[:date])
      if distribution[:per_share_usd]
        return 0.to_d unless distribution[:fx_rate]

        distribution[:per_share_usd] * units / distribution[:fx_rate]
      elsif distribution[:gross_eur] && distribution[:units_outstanding].positive?
        distribution[:gross_eur] * units / distribution[:units_outstanding]
      else
        0.to_d
      end
    end

    def finalize_worksheets
      @disposals.each { |row| row[:refused] = refused?(row[:symbol]) }
      @income.each { |row| row[:refused] = row[:symbol] ? refused?(row[:symbol]) : false }
      @disposals.sort_by! { |row| [row[:sold_on], row[:symbol].to_s] }
      @income.sort_by! { |row| [row[:date], row[:symbol].to_s, row[:activity_type].to_s] }
    end

    def build_result
      unclassified = @states.values.filter_map do |state|
        state[:symbol] unless state[:classification].kind.present?
      end.sort
      summaries_available = unclassified.empty?

      result = {
        year: @year,
        exchange_name: @exchange.name,
        summaries_available: summaries_available,
        kap: (aggregate_kap if summaries_available),
        kap_inv: (aggregate_kap_inv if summaries_available),
        symbols: symbol_rows,
        unclassified_symbols: unclassified,
        refused_symbols: @refusals.select { |_symbol, reasons| reasons.any? }.keys.sort,
        disposals: @disposals,
        income: @income,
        fee_total_eur: @fee_total_eur,
        warnings: @warnings
      }

      # `summaries_available` is about classification alone. `complete` is the one to banner on: a
      # cash leg has no symbol to refuse, so a missing rate on it leaves Z19 short with nothing but a
      # warning to show for it, and a report whose every security is refused still returns an
      # all-zero KAP. Keying a renderer on `summaries_available` would print both as finished.
      #
      # Assigned after the literal, never inside it: aggregators append warnings (aggregate_kap
      # does), so computing this in place would freeze it before the last warning landed — silently,
      # and only for whichever aggregator someone adds next.
      result[:complete] = @warnings.empty?
      result
    end

    def aggregate_kap
      total = @cash_kap.dup
      credited_despite_refusal = []

      @states.each_value do |state|
        unless refused?(state[:symbol])
          total.each_key { |key| total[key] += state[:kap][key] }
          next
        end

        # A refused symbol's basis figures are unsafe, but its withholding is not: a §56 transition
        # gap or an unmodelled corporate action says nothing about whether foreign tax was withheld.
        # Z41 is a locked sum over every withholding entry, and dropping a credit costs the taxpayer
        # money in the one direction they cannot detect. The entry's own leg is still the gate —
        # handle_withholding only accrues when that leg's rate resolved.
        next unless state[:kap][:z41_withholding_tax].nonzero?

        total[:z41_withholding_tax] += state[:kap][:z41_withholding_tax]
        credited_despite_refusal << state[:symbol]
      end

      if credited_despite_refusal.any?
        add_warning(code: :withholding_credited_for_refused_symbol, symbol: nil,
                    detail: credited_despite_refusal.sort)
      end
      total
    end

    def aggregate_kap_inv
      @states.each_value.with_object({}) do |state, totals|
        next unless state[:kind] == :fund && state[:fund_category]
        next if refused?(state[:symbol])

        category = totals[state[:fund_category]] ||= empty_kap_inv
        category.each_key { |key| category[key] += state[:kap_inv][key] }
      end
    end

    def symbol_rows
      @states.values.map do |state|
        reasons = REFUSAL_REASON_ORDER.select { |reason| @refusals[state[:symbol]].include?(reason) }
        {
          symbol: state[:symbol],
          kind: state[:kind],
          fund_category: state[:fund_category],
          classified: state[:classification].kind.present?,
          persisted: state[:classification].persisted?,
          refused: reasons.any?,
          refusal_reasons: reasons
        }
      end
    end

    def empty_kap
      {
        z19_saldo: 0.to_d,
        z20_share_gains: 0.to_d,
        z22_other_losses: 0.to_d,
        z23_share_losses: 0.to_d,
        z41_withholding_tax: 0.to_d
      }
    end

    def empty_kap_inv
      { distributions: 0.to_d, vorabpauschale: 0.to_d, sale_result: 0.to_d }
    end

    def refuse_symbol(symbol, code, detail:)
      @refusals[symbol] << code
      add_warning(code: code, symbol: symbol, detail: detail)
    end

    def refused?(symbol)
      @refusals[symbol].any?
    end

    def add_warning(code:, symbol:, detail:)
      key = [code, symbol, detail]
      return if @warning_keys.include?(key)

      @warning_keys << key
      @warnings << { code: code, symbol: symbol, detail: detail }
    end

    def report_year?(record)
      record.transacted_at.utc.year == @year
    end

    # Rows imported before raw_data got its {} default can carry nil, and one of those must not take
    # the whole report down over a description field.
    def raw_data(record)
      record.raw_data || {}
    end

    # `Exchanges::Alpaca#normalize_non_trade` stores an absolute `base_amount` — right for the
    # tracker's display and harmless to the crypto engines, but it strips the sign off a correction,
    # and a reversal added to Z19 as income is a wrong figure on a signed form. Every row that method
    # normalises is affected, so this is used by BOTH income legs of Z19: dividends and interest
    # (`INT`/`PTR` route through the same normaliser). The activity's own net_amount keeps the sign.
    # Read here rather than changing the ledger's semantics: this report is the first place the sign
    # reaches a tax figure. Withholding and fees deliberately keep `.abs` — a credit and a
    # disclosed-only aggregate are unsigned by nature.
    def signed_usd(record)
      net_amount = raw_data(record)['net_amount']
      net_amount.present? ? net_amount.to_d : record.base_amount.to_d
    end

    def record_date(record)
      record.transacted_at.utc.to_date
    end

    def activity_type(record)
      raw_data(record)['activity_type'] || record.entry_type
    end
  end
end
