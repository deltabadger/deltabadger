require 'csv'

module Tax
  class Report
    attr_reader :country_code, :jurisdiction, :year, :transactions

    def initialize(country:, year:, transactions:, stablecoin_as_fiat: false)
      @country_code = country
      @jurisdiction = Tax::Jurisdictions.for(country)
      raise ArgumentError, "Unknown country: #{country}" unless @jurisdiction

      @year = year
      @transactions = transactions
      @stablecoin_as_fiat = stablecoin_as_fiat
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
        results = method_class.new.calculate(enriched, **calculation_options)
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
        CSV.generate do |csv|
          csv << csv_headers
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
          append_warnings(csv) if @price_service.warnings.any?
        end
      end
    end

    def currency
      jurisdiction.dig(:currency_by_year, year) || jurisdiction[:currency]
    end

    private

    def scoped_transactions
      # Include all transactions up to end of year (for cost basis from prior years)
      # but only report disposals within the target year
      transactions.where(transacted_at: ..Time.utc(year + 1)).order(transacted_at: :asc)
    end

    def csv_headers
      if wealth_snapshot?
        return %w[reference_date asset amount value currency].map { |k| I18n.t("tax_report.headers.#{k}") }
      elsif pvct?
        keys = %w[date asset amount proceeds total_acquisition_cost portfolio_value gain_loss currency fee exchange tx_id]
      elsif weighted_average?
        keys = %w[date asset amount proceeds cost_basis gain_loss currency fee exchange tx_id cost_basis_complete]
      else
        keys = %w[date acquisition_date asset amount proceeds cost_basis gain_loss currency holding_days fee exchange tx_id
                  cost_basis_complete]
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
          disposal[:tx_id]
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
          disposal[:cost_basis_complete]
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
        disposal[:cost_basis_complete]
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

    def apply_danish_wash_sale(disposals, enriched)
      buys_by_asset = enriched
                      .select { |tx| ACQUISITION_ENTRY_TYPES.include?(tx[:entry_type].to_s) }
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

    def append_warnings(csv)
      csv << []
      csv << ["WARNING: #{I18n.t('tax_report.warnings.missing_prices')}:"]
      @price_service.warnings.uniq.each { |w| csv << [w] }
      csv << [I18n.t('tax_report.warnings.upgrade_hint')]
    end
  end
end
