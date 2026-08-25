require 'csv'

module Tax
  class Report
    attr_reader :country_code, :jurisdiction, :year, :transactions

    def initialize(country:, year:, transactions:, stablecoin_as_fiat: false, sync_issues: [])
      @country_code = country
      @jurisdiction = Tax::Jurisdictions.for(country)
      raise ArgumentError, "Unknown country: #{country}" unless @jurisdiction

      @year = year
      @transactions = transactions
      @stablecoin_as_fiat = stablecoin_as_fiat
      @sync_issues = sync_issues
    end

    def to_csv(&on_progress)
      @price_service = Tax::PriceService.new
      method_class = Tax::Jurisdictions.method_class(jurisdiction[:method])

      if wealth_snapshot?
        # Wealth snapshot doesn't need per-transaction price enrichment
        raw_transactions = scoped_transactions.map do |tx|
          { entry_type: tx.entry_type, base_currency: tx.base_currency, base_amount: tx.base_amount.to_d,
            transacted_at: tx.transacted_at }
        end
        on_progress&.call(50, 100)
        results = method_class.new.calculate(raw_transactions, **calculation_options)
        on_progress&.call(100, 100)
      else
        enriched = @price_service.enrich(scoped_transactions, currency: currency, &on_progress)
        # Fiat deposits are bank funding: no cost basis to assume and no withdrawal to link them to.
        # Naming them would bury the crypto rows this warning exists to surface.
        # A figure the user typed. It is theirs, not the venue's and not ours, and a tax document has
        # to say which rows rest on it — the same disclosure the assumed-basis deposits get.
        @stated_values = enriched.select { |tx| tx[:stated_value] }
        @assumed_deposits = enriched.select do |tx|
          tx[:entry_type].to_s == 'deposit' && !tx[:linked] &&
            !Tax::PriceService::FIAT_CURRENCIES.include?(tx[:base_currency])
        end
        # `taxable_entries` drops every fiat-base row before the engines, and income rows are frequently
        # fiat-base (Alpaca books dividends and `INT` as `other_income` with `base_currency: 'USD'`,
        # Kraken maps `dividend` the same way), so filtering here would silently drop every dividend and
        # interest payment from the report.
        income_entries = enriched.select do |tx|
          INCOME_TYPES.include?(tx[:entry_type].to_s) && tx[:transacted_at].utc.year == year
        end
        # Value them here rather than at render time: a fiat income row's FX lookup is the only place
        # a missing rate for it can surface (enrich short-circuits a fiat base to zero), and the
        # incomplete banner is decided — and counted — before the income section is ever written.
        @income_rows = income_entries.map { |tx| [tx, income_value(tx)] }
        results = method_class.new.calculate(taxable_entries(enriched), **calculation_options)
      end

      unless wealth_snapshot?
        # Filter to only disposals within the target year (prior years used for cost basis only)
        results.select! { |d| d[:date].year == year }

        apply_holding_exemption(results) if jurisdiction[:holding_exemption]
        apply_short_long_term(results) if jurisdiction[:short_long_term]
        apply_tax_rate(results) if jurisdiction[:tax_rate]
        apply_holding_tax_rate(results) if jurisdiction[:holding_tax_rate]
        apply_exemption_threshold(results) if jurisdiction[:exemption_threshold]
        apply_expense_deduction(results) if jurisdiction[:expense_deduction]
        apply_danish_wash_sale(results, enriched) if jurisdiction[:danish_wash_sale]
        apply_czech_exemptions(results) if jurisdiction[:czech_exemptions]
      end

      I18n.with_locale(jurisdiction[:locale]) do
        CsvSafe.generate do |csv|
          csv << csv_headers
          @sync_issues.each { |issue| csv << [sync_issue_banner(issue)] }
          csv << [incomplete_banner] if @price_service.warnings.any?
          if results.empty?
            csv << [I18n.t('tax_report.no_taxable_transactions')]
          else
            results.each { |d| csv << csv_row(d) }
            append_loss_summary(csv, results) if jurisdiction[:loss_deduction_rate]
            append_irish_summary(csv, results) if jurisdiction[:annual_exemption]
            append_expense_deduction_summary(csv, results) if jurisdiction[:expense_deduction]
            append_flat_tax_summary(csv, results) if jurisdiction[:flat_tax_rate]
            append_danish_summary(csv, results) if jurisdiction[:per_asset_summary]
            append_czech_summary(csv, results) if jurisdiction[:czech_exemptions]
          end
          append_income_section(csv) if @income_rows.present?
          append_warnings(csv) if @price_service.warnings.any?
          append_deposit_basis_warnings(csv) if @assumed_deposits.present?
          append_stated_values(csv) if @stated_values.present?
          append_income_disclosure(csv) if jurisdiction[:income_taxed_separately]
        end
      end
    end

    def currency
      jurisdiction.dig(:currency_by_year, year) || jurisdiction[:currency]
    end

    private

    def sync_issue_banner(issue)
      message = if issue[:reason] == :never_synced
                  I18n.t('tax_report.warnings.exchange_never_synced', exchange: issue[:exchange])
                else
                  I18n.t('tax_report.warnings.exchange_sync_failed', exchange: issue[:exchange])
                end
      "#{I18n.t('tax_report.incomplete_banner_prefix')}: #{message}"
    end

    def scoped_transactions
      # Include all transactions up to end of year (for cost basis from prior years)
      # but only report disposals within the target year
      transactions.where(transacted_at: ..Time.utc(year + 1)).order(transacted_at: :asc)
    end

    # A fiat ledger row is one leg of a trade or bank funding, never a disposal or a lot. Kraken
    # emits one signed row per asset, so a EUR-funded buy arrives as a `sell` of EUR; every engine
    # priced it at 1.0 against lots the user never had. Filter after enrichment on purpose: the fiat
    # row carries the trade's fee, which PriceService#attribute_quote_row_fees moves onto the crypto leg first.
    def taxable_entries(enriched)
      enriched.reject { |tx| Tax::PriceService::FIAT_CURRENCIES.include?(tx[:base_currency]) }
    end

    def csv_headers
      if wealth_snapshot?
        return %w[reference_date asset amount value currency].map { |k| I18n.t("tax_report.headers.#{k}") }
      elsif pvct?
        keys = %w[date asset amount proceeds total_acquisition_cost portfolio_value gain_loss currency fee exchange tx_id
                  data_incomplete]
      elsif weighted_average?
        keys = %w[date asset amount proceeds cost_basis gain_loss currency fee exchange tx_id cost_basis_complete
                  data_incomplete]
      else
        keys = %w[date acquisition_date asset amount proceeds cost_basis gain_loss currency holding_days fee exchange tx_id
                  cost_basis_complete data_incomplete]
        keys << 'tax_exempt' if jurisdiction[:holding_exemption]
        keys << 'old_stock' if jurisdiction[:old_stock_cutoff]
        keys << 'term' if jurisdiction[:short_long_term]
        keys << 'matching_rule' if jurisdiction[:method].in?(%i[share_pooling fifo_4week])
        keys << 'period' if jurisdiction[:split_payment]
        keys << 'tax_rate' if jurisdiction[:tax_rate] || jurisdiction[:holding_tax_rate]
        keys << 'exempt' if jurisdiction[:exemption_threshold]
        keys << 'loss_denied' if jurisdiction[:danish_wash_sale]
        keys.push('tax_exempt', 'exempt_reason') if jurisdiction[:czech_exemptions]
      end

      keys.map { |k| I18n.t("tax_report.headers.#{k}") }
    end

    def csv_row(disposal)
      if wealth_snapshot?
        wealth_snapshot_row(disposal)
      elsif disposal[:type] == :summary
        wealth_snapshot_row(disposal) # reuse for summary rows
      elsif weighted_average?
        weighted_average_row(disposal)
      elsif pvct?
        [
          disposal[:date].utc.strftime('%Y-%m-%dT%H:%M:%SZ'),
          disposal[:asset],
          disposal[:amount],
          disposal[:proceeds]&.round(2),
          disposal[:total_acquisition_cost]&.round(2),
          disposal[:portfolio_value]&.round(2),
          disposal[:gain_loss]&.round(2),
          currency,
          disposal[:fee]&.round(2),
          disposal[:exchange],
          disposal[:tx_id],
          disposal[:data_incomplete]
        ]
      else
        row = [
          disposal[:date].utc.strftime('%Y-%m-%dT%H:%M:%SZ'),
          disposal[:acquisition_date]&.utc&.strftime('%Y-%m-%dT%H:%M:%SZ'),
          disposal[:asset],
          disposal[:amount],
          disposal[:proceeds]&.round(2),
          disposal[:cost_basis]&.round(2),
          disposal[:gain_loss]&.round(2),
          currency,
          disposal[:holding_days],
          disposal[:fee]&.round(2),
          disposal[:exchange],
          disposal[:tx_id],
          disposal[:cost_basis_complete],
          disposal[:data_incomplete]
        ]
        row << disposal[:tax_exempt] if jurisdiction[:holding_exemption]
        row << disposal[:old_stock] if jurisdiction[:old_stock_cutoff]
        row << I18n.t("tax_report.values.term_#{disposal[:term]}") if jurisdiction[:short_long_term]
        row << disposal[:matching_rule] if jurisdiction[:method].in?(%i[share_pooling fifo_4week])
        row << disposal[:period] if jurisdiction[:split_payment]
        row << disposal[:tax_rate] if jurisdiction[:tax_rate] || jurisdiction[:holding_tax_rate]
        row << disposal[:exempt] if jurisdiction[:exemption_threshold]
        row << (disposal[:loss_denied] ? I18n.t('tax_report.summary.denied_losses') : nil) if jurisdiction[:danish_wash_sale]
        row.push(disposal[:tax_exempt], disposal[:exempt_reason]) if jurisdiction[:czech_exemptions]
        row
      end
    end

    def calculation_options
      opts = { price_service: @price_service, currency: currency }
      opts[:crypto_to_crypto_taxable] = jurisdiction[:crypto_to_crypto_taxable] if jurisdiction.key?(:crypto_to_crypto_taxable)
      opts[:stablecoin_as_fiat] = @stablecoin_as_fiat
      opts[:old_stock_cutoff] = jurisdiction[:old_stock_cutoff] if jurisdiction[:old_stock_cutoff]
      opts[:swap_resets_holding_period] = true if jurisdiction[:swap_resets_holding_period]
      opts[:wealth_tax] = jurisdiction[:wealth_tax] if jurisdiction[:wealth_tax]
      opts[:snapshot_date] = jurisdiction[:snapshot_date] if jurisdiction[:snapshot_date]
      opts[:summary_only_total] = jurisdiction[:summary_only_total] if jurisdiction[:summary_only_total]
      opts[:year] = year
      opts
    end

    def pvct?
      jurisdiction[:method] == :pvct
    end

    def weighted_average?
      jurisdiction[:method] == :weighted_average
    end

    def wealth_snapshot?
      jurisdiction[:method] == :wealth_snapshot
    end

    def weighted_average_row(disposal)
      [
        disposal[:date].utc.strftime('%Y-%m-%dT%H:%M:%SZ'),
        disposal[:asset],
        disposal[:amount],
        disposal[:proceeds]&.round(2),
        disposal[:cost_basis]&.round(2),
        disposal[:gain_loss]&.round(2),
        currency,
        disposal[:fee]&.round(2),
        disposal[:exchange],
        disposal[:tx_id],
        disposal[:cost_basis_complete],
        disposal[:data_incomplete]
      ]
    end

    def append_loss_summary(csv, results)
      gains = results.select { |d| d[:gain_loss]&.positive? }.sum { |d| d[:gain_loss] }
      losses = results.select { |d| d[:gain_loss]&.negative? }.sum { |d| d[:gain_loss] }.abs
      rate = jurisdiction[:loss_deduction_rate]
      deductible = (losses * rate).round(2)

      csv << []
      csv << [I18n.t('tax_report.summary.total_gains'), gains.round(2)]
      csv << [I18n.t('tax_report.summary.total_losses'), losses.round(2),
              "#{(rate * 100).to_i}% #{I18n.t('tax_report.summary.deductible')}", deductible]
    end

    def wealth_snapshot_row(row)
      if row[:type] == :holding
        [
          row[:date].utc.strftime('%Y-%m-%d'),
          row[:asset],
          row[:amount],
          row[:value],
          currency
        ]
      else
        # Summary row
        label = I18n.t("tax_report.summary.#{row[:label]}")
        label = "#{label} (#{row[:rate]})" if row[:rate]
        [nil, nil, label, row[:value], nil]
      end
    end

    def apply_holding_exemption(disposals)
      threshold_days = (jurisdiction[:holding_exemption] / 1.day).to_i
      disposals.each do |d|
        d[:tax_exempt] = d[:holding_days].present? && d[:holding_days] > threshold_days
      end
    end

    def apply_short_long_term(disposals)
      disposals.each do |d|
        d[:term] = if d[:holding_days].present? && d[:holding_days] > 365
                     'long'
                   else
                     'short'
                   end
      end
    end

    def apply_tax_rate(disposals)
      rate_config = jurisdiction[:tax_rate]
      cutoff = rate_config[:cutoff]
      disposals.each do |d|
        d[:tax_rate] = d[:date] < cutoff ? rate_config[:before] : rate_config[:after]
      end
    end

    def apply_exemption_threshold(disposals)
      config = jurisdiction[:exemption_threshold]

      # Group disposals by year and check threshold per year
      by_year = disposals.group_by { |d| d[:date].year }
      by_year.each do |disposal_year, year_disposals|
        if disposal_year <= config[:max_year]
          total_gains = year_disposals.select { |d| d[:gain_loss].positive? }.sum { |d| d[:gain_loss] }
          exempt = total_gains < config[:amount]
          year_disposals.each { |d| d[:exempt] = exempt }
        else
          year_disposals.each { |d| d[:exempt] = false }
        end
      end
    end

    def append_irish_summary(csv, results)
      exemption = jurisdiction[:annual_exemption]
      gains = results.select { |d| d[:gain_loss]&.positive? }.sum { |d| d[:gain_loss] }
      losses = results.select { |d| d[:gain_loss]&.negative? }.sum { |d| d[:gain_loss] }.abs
      net_gains = [gains - losses, 0].max
      taxable = [net_gains - exemption, 0].max
      cgt = (taxable * 0.33).round(2)

      initial_gains = results.select { |d| d[:period] == 'initial' && d[:gain_loss]&.positive? }
                             .sum { |d| d[:gain_loss] }
      later_gains = results.select { |d| d[:period] == 'later' && d[:gain_loss]&.positive? }
                           .sum { |d| d[:gain_loss] }
      initial_proportion = net_gains.positive? ? (initial_gains / (initial_gains + later_gains)) : 1
      later_proportion = 1 - initial_proportion

      csv << []
      csv << [I18n.t('tax_report.summary.total_gains'), gains.round(2)]
      csv << [I18n.t('tax_report.summary.total_losses'), losses.round(2)]
      csv << [I18n.t('tax_report.summary.annual_exemption'), exemption]
      csv << [I18n.t('tax_report.summary.taxable_gains'), taxable.round(2)]
      csv << [I18n.t('tax_report.summary.cgt_33'), cgt]
      csv << ["  #{I18n.t('tax_report.summary.due_dec_15')}",
              (cgt * initial_proportion).round(2)]
      csv << ["  #{I18n.t('tax_report.summary.due_jan_31')}",
              (cgt * later_proportion).round(2)]
    end

    def apply_expense_deduction(disposals)
      rate = jurisdiction[:expense_deduction]
      disposals.each do |d|
        next unless d[:gain_loss]&.positive?

        d[:expense_deduction] = (d[:gain_loss] * rate).round(2)
        d[:gain_loss] = (d[:gain_loss] - d[:expense_deduction]).round(2)
      end
    end

    def append_expense_deduction_summary(csv, results)
      rate = jurisdiction[:expense_deduction]
      gains = results.select { |d| d[:gain_loss]&.positive? || d[:expense_deduction]&.positive? }
                     .sum { |d| (d[:gain_loss] || 0) + (d[:expense_deduction] || 0) }
      losses = results.select { |d| d[:gain_loss]&.negative? }.sum { |d| d[:gain_loss] }.abs
      total_deduction = results.sum { |d| d[:expense_deduction] || 0 }
      net = [gains - losses - total_deduction, 0].max
      tax = (net * 0.1).round(2)

      csv << []
      csv << [I18n.t('tax_report.summary.total_gains'), gains.round(2)]
      csv << [I18n.t('tax_report.summary.total_losses'), losses.round(2)]
      csv << [I18n.t('tax_report.summary.expense_deduction'),
              total_deduction.round(2), "#{(rate * 100).to_i}%"]
      csv << [I18n.t('tax_report.summary.taxable_income'), net.round(2)]
      csv << [I18n.t('tax_report.summary.tax_10'), tax]
    end

    def append_flat_tax_summary(csv, results)
      rate = jurisdiction[:flat_tax_rate]
      gains = results.select { |d| d[:gain_loss]&.positive? }.sum { |d| d[:gain_loss] }
      losses = results.select { |d| d[:gain_loss]&.negative? }.sum { |d| d[:gain_loss] }.abs
      net = [gains - losses, 0].max
      tax = (net * rate).round(2)
      pct = (rate * 100).to_i

      csv << []
      csv << [I18n.t('tax_report.summary.total_gains'), gains.round(2)]
      csv << [I18n.t('tax_report.summary.total_losses'), losses.round(2)]
      csv << [I18n.t('tax_report.summary.taxable_income'), net.round(2)]
      csv << [I18n.t('tax_report.summary.tax_percent', pct: pct), tax]
    end

    def apply_holding_tax_rate(disposals)
      config = jurisdiction[:holding_tax_rate]
      disposals.each { |d| d[:tax_rate] = config[d[:term].to_sym] }
    end

    def apply_czech_exemptions(disposals)
      config = jurisdiction[:czech_exemptions]
      threshold_days = (config[:time_test] / 1.day).to_i

      total_proceeds = disposals.sum { |d| d[:proceeds] || 0 }
      value_test_passed = total_proceeds <= config[:value_test]

      disposals.each do |d|
        if value_test_passed
          d[:tax_exempt] = true
          d[:exempt_reason] = 'hodnotový test'
        elsif d[:holding_days].present? && d[:holding_days] > threshold_days
          d[:tax_exempt] = true
          d[:exempt_reason] = 'časový test'
        else
          d[:tax_exempt] = false
          d[:exempt_reason] = nil
        end
      end
    end

    def append_czech_summary(csv, results)
      total_proceeds = results.sum { |d| d[:proceeds] || 0 }
      total_costs = results.sum { |d| d[:cost_basis] || 0 }
      total_gain = results.sum { |d| d[:gain_loss] || 0 }

      exempt_time = results.select { |d| d[:tax_exempt] && d[:exempt_reason] == 'časový test' && d[:gain_loss]&.positive? }
                           .sum { |d| d[:gain_loss] }
      value_test_passed = results.any? { |d| d[:exempt_reason] == 'hodnotový test' }

      exempt_gains = results.select { |d| d[:tax_exempt] && d[:gain_loss]&.positive? }.sum { |d| d[:gain_loss] }
      taxable = [total_gain - exempt_gains, 0].max

      csv << []
      csv << [I18n.t('tax_report.summary.total_proceeds'), total_proceeds.round(2)]
      csv << [I18n.t('tax_report.summary.total_costs'), total_costs.round(2)]
      csv << [I18n.t('tax_report.summary.total_gain'), total_gain.round(2)]
      csv << [I18n.t('tax_report.summary.exempt_time_test'), exempt_time.round(2)]
      value_label = if value_test_passed
                      I18n.t('tax_report.summary.yes')
                    else
                      I18n.t('tax_report.summary.no')
                    end
      csv << [I18n.t('tax_report.summary.exempt_value_test'), value_label]
      csv << [I18n.t('tax_report.summary.taxable_gain'), taxable.round(2)]
    end

    ACQUISITION_ENTRY_TYPES = %w[buy deposit swap_in staking_reward lending_interest airdrop mining other_income].freeze
    INCOME_TYPES = %w[staking_reward lending_interest airdrop mining other_income].freeze

    def apply_danish_wash_sale(disposals, enriched)
      # A linked deposit is the far end of the user's own transfer, not a repurchase — counting it
      # here would deny a real loss because the coins moved between two of their own accounts.
      buys_by_asset = enriched
                      .select { |tx| ACQUISITION_ENTRY_TYPES.include?(tx[:entry_type].to_s) && !tx[:linked] }
                      .group_by { |tx| tx[:base_currency] }

      disposals.each do |d|
        next unless d[:gain_loss]&.negative?
        next unless d[:acquisition_date] && d[:date]

        asset_buys = buys_by_asset[d[:asset]] || []
        d[:loss_denied] = asset_buys.any? do |buy|
          buy[:transacted_at] > d[:acquisition_date] && buy[:transacted_at] < d[:date]
        end
      end
    end

    def append_danish_summary(csv, results)
      rate = jurisdiction[:loss_deduction_rate_on_losses]
      by_asset = results.group_by { |d| d[:asset] }

      by_asset.each do |asset, disposals|
        gains = disposals.select { |d| d[:gain_loss]&.positive? }.sum { |d| d[:gain_loss] }
        allowed_losses = disposals.select { |d| d[:gain_loss]&.negative? && !d[:loss_denied] }
                                  .sum { |d| d[:gain_loss] }.abs
        denied_losses = disposals.select { |d| d[:gain_loss]&.negative? && d[:loss_denied] }
                                 .sum { |d| d[:gain_loss] }.abs
        deduction = (allowed_losses * rate).round(2)

        csv << []
        csv << ["#{asset}:"]
        csv << ["  #{I18n.t('tax_report.summary.gains_box_20')}", gains.round(2)]
        csv << ["  #{I18n.t('tax_report.summary.losses_box_58')}", allowed_losses.round(2),
                "#{(rate * 100).to_i}% #{I18n.t('tax_report.summary.deduction')}", deduction]
        csv << ["  #{I18n.t('tax_report.summary.denied_losses')}", denied_losses.round(2)] if denied_losses.positive?
      end
    end

    # First data row, not a header row, so CSV.parse(csv, headers: true) still sees the real columns.
    def incomplete_banner
      "#{I18n.t('tax_report.incomplete_banner_prefix')}: " \
        "#{I18n.t('tax_report.incomplete_banner', missing: @price_service.warnings.uniq.size)}"
    end

    def append_warnings(csv)
      csv << []
      csv << ["WARNING: #{I18n.t('tax_report.warnings.missing_prices')}:"]
      @price_service.warnings.uniq.each { |w| csv << [w] }
      csv << [I18n.t('tax_report.warnings.upgrade_hint')]
    end

    # An unlinked deposit's cost basis is the market value on the day it arrived — a disclosed
    # assumption, not a fact. Naming the rows lets the user link each one to its withdrawal in
    # the tracker and get the real basis.
    def append_deposit_basis_warnings(csv)
      csv << []
      csv << ["WARNING: #{I18n.t('tax_report.warnings.deposit_basis_assumed')}:"]
      @assumed_deposits.each do |tx|
        csv << ["#{tx[:base_currency]} #{tx[:base_amount]} #{tx[:transacted_at].utc.strftime('%Y-%m-%d')}"]
      end
      csv << [I18n.t('tax_report.warnings.deposit_basis_assumed_hint')]
    end

    # Values the user stated by hand, on rows the venue priced badly or not at all. Named so they can
    # be checked one by one — they are the user's own figures, and the report says so rather than
    # passing them off as the exchange's.
    def append_stated_values(csv)
      csv << []
      csv << ["NOTE: #{I18n.t('tax_report.warnings.stated_values')}:"]
      @stated_values.each do |tx|
        csv << ["#{tx[:base_currency]} #{tx[:base_amount]} #{tx[:transacted_at].utc.strftime('%Y-%m-%d')}",
                tx[:fiat_value].round(2)]
      end
      csv << [I18n.t('tax_report.warnings.stated_values_hint')]
    end

    # Staking, interest, airdrops and mining are taxable when received, on top of any gain on a later
    # sale. The engines only ever emit disposals, so without this section an entire category of
    # taxable income leaves no trace in the report.
    def append_income_section(csv)
      csv << []
      csv << [I18n.t('tax_report.income.header')]
      @income_rows.each do |tx, value|
        csv << [tx[:transacted_at].utc.strftime('%Y-%m-%d'), income_type_label(tx), tx[:base_currency],
                tx[:base_amount], value.round(2), currency]
      end
      @income_rows.group_by { |tx, _value| tx[:entry_type].to_s }.each_value do |rows|
        csv << [nil, I18n.t('tax_report.income.total', type: income_type_label(rows.first.first)), nil, nil,
                rows.sum { |_tx, value| value }.round(2), currency]
      end
    end

    # A wealth snapshot has no enriched rows to list income from, but Switzerland taxes that income
    # all the same — say so rather than let the omission read as "nothing to declare".
    def append_income_disclosure(csv)
      csv << []
      csv << [I18n.t('tax_report.income.not_included')]
    end

    def income_type_label(entry)
      I18n.t("tax_report.income.types.#{entry[:entry_type]}")
    end

    # PriceService zeroes `fiat_value` for a fiat base on purpose — no engine consumes it and pricing
    # it could only fabricate a missing-price warning. An income row often IS fiat-based (a USD
    # dividend), and here the amount is the whole point, so convert it at the day's FX rate.
    def income_value(entry)
      return entry[:fiat_value] unless Tax::PriceService::FIAT_CURRENCIES.include?(entry[:base_currency])

      @price_service.convert_fiat(amount: entry[:base_amount], from: entry[:base_currency],
                                  to: currency, timestamp: entry[:transacted_at])
    end
  end
end
