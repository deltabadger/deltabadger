require 'csv'

module Tax
  class BrokerReportCsv
    KAP_INV_ZEILEN = {
      equity_fund: { distributions: '4', vorabpauschale: '9', sale_result: '14' },
      mixed_fund: { distributions: '5', vorabpauschale: '10', sale_result: '17' },
      real_estate_fund: { distributions: '6', vorabpauschale: '11', sale_result: '20' },
      foreign_real_estate_fund: { distributions: '7', vorabpauschale: '12', sale_result: '23' },
      other_fund: { distributions: '8', vorabpauschale: '13', sale_result: '26' }
    }.freeze

    def initialize(result)
      @result = result
    end

    def to_csv
      CsvSafe.generate do |csv|
        append_head(csv)
        append_kap(csv) if @result[:kap]
        append_kap_inv(csv) if @result[:kap_inv]&.any?
        append_disposals(csv) if @result[:disposals].any?
        append_income(csv) if @result[:income].any? || @result[:fee_total_eur].nonzero?
        append_warnings(csv) if @result[:unclassified_symbols].any? || @result[:warnings].any?
        append_disclosures(csv)
      end
    end

    private

    def append_head(csv)
      csv << [t('tax_report.broker.title'), @result[:exchange], @result[:year]]
      csv << [warn_prefixed(t('tax_report.broker.unclassified_banner'))] unless @result[:summaries_available]
      csv << [warn_prefixed(t('tax_report.broker.incomplete_banner'))] unless @result[:complete]
    end

    def append_kap(csv)
      kap = @result[:kap]
      csv << []
      csv << [t('tax_report.broker.sections.kap')]
      csv << [
        t('tax_report.broker.headers.zeile'),
        t('tax_report.broker.headers.description'),
        t('tax_report.broker.headers.amount_eur')
      ]
      csv << ['19', t('tax_report.broker.kap.z19'), eur(kap[:z19_saldo])]
      csv << ['20', t('tax_report.broker.kap.z20'), eur(kap[:z20_share_gains])]
      csv << ['22', t('tax_report.broker.kap.z22'), eur(kap[:z22_other_losses])]
      csv << ['23', t('tax_report.broker.kap.z23'), eur(kap[:z23_share_losses])]
      csv << ['41', t('tax_report.broker.kap.z41'), eur(kap[:z41_withholding_tax])]
    end

    def append_kap_inv(csv)
      csv << []
      csv << [t('tax_report.broker.sections.kap_inv')]
      csv << [
        t('tax_report.broker.headers.zeile'),
        t('tax_report.broker.headers.fund_category'),
        t('tax_report.broker.headers.position'),
        t('tax_report.broker.headers.amount_eur')
      ]

      # A category with no Zeile mapping would silently vanish from a signed form. This report
      # refuses rather than under-reports everywhere else; it must not make an exception here.
      unmapped = @result[:kap_inv].keys - KAP_INV_ZEILEN.keys
      raise ArgumentError, "No KAP-INV Zeile mapping for fund category: #{unmapped.join(', ')}" if unmapped.any?

      KAP_INV_ZEILEN.each do |category, positions|
        values = @result[:kap_inv][category]
        next unless values

        positions.each do |position, zeile|
          csv << [
            zeile,
            t("tax_report.broker.fund_categories.#{category}"),
            t("tax_report.broker.kap_inv.#{position}"),
            eur(values[position])
          ]
        end
      end
    end

    def append_disposals(csv)
      csv << []
      csv << [t('tax_report.broker.sections.disposals')]
      csv << disposal_headers
      @result[:disposals].each do |disposal|
        csv << disposal_row(disposal)
        disposal[:matches].each { |match| csv << disposal_match_row(match) }
      end
    end

    def disposal_headers
      %w[
        symbol kind fund_category date units proceeds_usd fx_rate proceeds_eur fee_eur cost_eur
        vorabpauschale_eur gain_eur status
      ].map { |header| t("tax_report.broker.headers.#{header}") }
    end

    def disposal_row(disposal)
      [
        disposal[:symbol],
        translated_value('kinds', disposal[:kind]),
        translated_value('fund_categories', disposal[:fund_category]),
        disposal[:sold_on]&.to_s,
        disposal[:units],
        eur(disposal[:proceeds_usd]),
        disposal[:fx_rate],
        eur(disposal[:proceeds_eur]),
        eur(disposal[:fee_eur]),
        eur(disposal[:cost_eur]),
        eur(disposal[:vorabpauschale_deducted_eur]),
        eur(disposal[:gain_eur]),
        disposal[:refused] ? t('tax_report.broker.status.refused') : nil
      ]
    end

    def disposal_match_row(match)
      [
        nil,
        t('tax_report.broker.rows.lot'),
        nil,
        match[:acquired_on]&.to_s,
        match[:units],
        nil,
        match[:fx_rate],
        nil,
        nil,
        eur(match[:cost_eur]),
        eur(match[:vap_eur]),
        nil,
        match[:uncovered] ? t('tax_report.broker.status.uncovered') : nil
      ]
    end

    def append_income(csv)
      csv << []
      csv << [t('tax_report.broker.sections.income')]
      csv << income_headers
      @result[:income].each { |income| csv << income_row(income) }
      csv << fee_total_row if @result[:fee_total_eur].nonzero?
    end

    def income_headers
      %w[date symbol activity_type bucket amount_usd fx_rate amount_eur_income status]
        .map { |header| t("tax_report.broker.headers.#{header}") }
    end

    def income_row(income)
      [
        income[:date]&.to_s,
        income[:symbol],
        income[:activity_type],
        t("tax_report.broker.buckets.#{income[:bucket]}"),
        eur(income[:usd_amount]),
        income[:fx_rate],
        eur(income[:eur_amount]),
        income[:refused] ? t('tax_report.broker.status.refused') : nil
      ]
    end

    def fee_total_row
      [nil, nil, t('tax_report.broker.rows.fee_total'), nil, nil, nil, eur(@result[:fee_total_eur]), nil]
    end

    def append_warnings(csv)
      csv << []
      csv << [t('tax_report.broker.sections.warnings')]
      append_unclassified_warnings(csv) if @result[:unclassified_symbols].any?
      @result[:warnings].each { |warning| csv << [warning[:symbol], warning_message(warning)] }
    end

    def append_unclassified_warnings(csv)
      csv << [t('tax_report.broker.warnings.summaries_blocked')]
      @result[:unclassified_symbols].each do |symbol|
        csv << [symbol, t('tax_report.broker.warnings.unclassified')]
      end
    end

    def warning_message(warning)
      code = warning[:code]
      key = "tax_report.broker.warnings.#{code}"
      detail = format_detail(warning[:detail])
      return t(key, detail: detail) if I18n.exists?(key)

      t('tax_report.broker.warnings.unknown', code: code, detail: detail)
    end

    # A warning detail is a date, a year, an activity code or a list of symbols, and every one of
    # them belongs in the sentence as plain text.
    def format_detail(detail)
      detail.is_a?(Array) ? detail.join(', ') : detail.to_s
    end

    def append_disclosures(csv)
      csv << []
      csv << [t('tax_report.broker.sections.disclosures')]
      %w[
        not_tax_advice usd_balance_fx us_etf_default fx_source interest_withholding_net regulatory_fees
      ].each do |disclosure|
        csv << [t("tax_report.broker.disclosures.#{disclosure}")]
      end
    end

    def translated_value(scope, value)
      t("tax_report.broker.#{scope}.#{value}") if value
    end

    def warn_prefixed(text)
      "#{t('tax_report.incomplete_banner_prefix')}: #{text}"
    end

    # BigDecimal straight into the cell, as `Tax::Report` does. It is a non-String, so `CsvSafe`
    # leaves it alone and a negative amount stays a number instead of being escaped into text —
    # and it never round-trips through a binary float, which would print a unit count below
    # 0.0001 as `1.0e-05`. Rounding happens here and nowhere earlier.
    def eur(value)
      value&.round(2)
    end

    def t(key, **options)
      I18n.t(key, **options)
    end
  end
end
