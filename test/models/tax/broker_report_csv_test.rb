require 'test_helper'
require 'csv'

class Tax::BrokerReportCsvTest < ActiveSupport::TestCase
  setup do
    @user = create(:user)
    @exchange = create(:alpaca_exchange)
    @api_key = create(:api_key, user: @user, exchange: @exchange)
  end

  test 'clean share round trip renders KAP disposal lot and all disclosures without a warning banner' do
    clean_share_round_trip

    rows = CSV.parse(report(2024))

    assert_equal(['19', 'Foreign investment income', '589.0'], rows.find { |row| row[0] == '19' })
    assert_equal(['20', 'Gains from the disposal of shares included in line 19', '589.0'], rows.find { |row| row[0] == '20' })

    disposal = rows.find { |row| row[0] == 'ZQX' && row[3] == '2024-09-02' }
    assert_equal '589.0', disposal[11]
    assert_nil disposal[12]

    lot = rows.find { |row| row[1] == 'of which: acquisition' }
    assert_equal '2024-03-01', lot[3]
    assert_equal '910.0', lot[9]

    english_disclosures.each { |disclosure| assert_includes rows, [disclosure] }
    refute(rows.any? { |row| row[0]&.start_with?('WARNING: Report incomplete') })
  end

  test 'incomplete report banners and refuses a classified share while retaining worksheet evidence' do
    clean_share_round_trip
    seed_fx(Date.new(2024, 5, 1), '1.00')
    tx(entry_type: :unsupported_activity, base_currency: 'ZQX', base_amount: '1', at: Time.utc(2024, 5, 1),
       raw_data: { 'activity_type' => 'OPASN' })

    rows = CSV.parse(report(2024))

    assert_includes rows, ['WARNING: Report incomplete — review the warnings below; do not file as-is']
    assert_equal(['19', 'Foreign investment income', '0.0'], rows.find { |row| row[0] == '19' })
    assert_equal(['20', 'Gains from the disposal of shares included in line 19', '0.0'], rows.find { |row| row[0] == '20' })

    disposal = rows.find { |row| row[0] == 'ZQX' && row[3] == '2024-09-02' }
    assert_equal 'Not included in the summary — see warnings', disposal[12]
    assert_includes rows, ['ZQX', "Unsupported transaction (OPASN) — this security's amounts are not included in the summary."]
  end

  test 'unclassified symbol omits both summaries but preserves the to-do list and disposal worksheet' do
    seed_fx(Date.new(2024, 3, 1), '1.00')
    tx(entry_type: :buy, base_currency: 'UNK', base_amount: '10', quote_currency: 'USD',
       quote_amount: '1000', at: Time.utc(2024, 3, 1))
    tx(entry_type: :sell, base_currency: 'UNK', base_amount: '10', quote_currency: 'USD',
       quote_amount: '1500', at: Time.utc(2024, 3, 1, 12))

    broker_report = Tax::BrokerReport.new(user: @user, year: 2024, exchange: @exchange)
    assert broker_report.result[:complete]
    assert_empty broker_report.result[:warnings]

    rows = CSV.parse(broker_report.to_csv)

    refute_includes rows, ['SUMMARY — ANLAGE KAP']
    refute_includes rows, ['SUMMARY — ANLAGE KAP-INV']
    assert_includes rows, ['WARNING: Summaries not produced — not all securities are classified yet; see the warnings below']
    refute_includes rows, ['WARNING: Report incomplete — review the warnings below; do not file as-is']
    assert_includes rows, ['UNK', 'Not yet classified — specify the security type (share, fund, or other security).']
    assert_includes rows, ['WORKSHEET — DISPOSALS']
    assert(rows.any? { |row| row[0] == 'UNK' && row[3] == '2024-03-01' && row[11] == '500.0' })
  end

  test 'KAP-INV renders exactly the three rows for each present fund category' do
    seed_fx(Date.new(2024, 6, 3), '1.00')
    classify('FNDX', kind: :fund, fund_category: :other_fund)
    tx(entry_type: :other_income, base_currency: 'USD', base_amount: '200', quote_currency: 'FNDX',
       at: Time.utc(2024, 6, 3), raw_data: { 'activity_type' => 'DIV' })

    rows = CSV.parse(report(2024))
    section_index = rows.index(['SUMMARY — ANLAGE KAP-INV'])
    kap_inv_rows = rows.drop(section_index + 2).take_while(&:present?)

    assert_equal 3, kap_inv_rows.size
    assert_equal [
      ['8', 'Other funds', 'Distributions (before Teilfreistellung)', '200.0'],
      ['13', 'Other funds', 'Vorabpauschale (before Teilfreistellung)', '0.0'],
      ['26', 'Other funds', 'Result from disposal (before Teilfreistellung)', '0.0']
    ], kap_inv_rows
    refute rows.flatten.include?('Equity funds')
  end

  test 'cash-level missing FX banners an otherwise classified report with a blank warning symbol' do
    clean_share_round_trip
    tx(entry_type: :other_income, base_currency: 'USD', base_amount: '50', at: Time.utc(2024, 11, 20),
       raw_data: { 'activity_type' => 'INT' })

    rows = CSV.parse(report(2024))

    assert_includes rows, ['WARNING: Report incomplete — review the warnings below; do not file as-is']
    assert_equal(['20', 'Gains from the disposal of shares included in line 19', '589.0'], rows.find { |row| row[0] == '20' })
    assert_includes rows, [nil, 'No ECB reference rate available for 2024-11-20 — the affected amount is missing from the summary.']
  end

  test 'income worksheet labels dividend and withholding buckets and includes the regulatory fee aggregate' do
    seed_fx(Date.new(2024, 6, 3), '1.00')
    classify('ZQX', kind: :share)
    tx(entry_type: :other_income, base_currency: 'USD', base_amount: '100', quote_currency: 'ZQX',
       at: Time.utc(2024, 6, 3), raw_data: { 'activity_type' => 'DIV' })
    tx(entry_type: :withholding_tax, base_currency: 'USD', base_amount: '15', quote_currency: 'ZQX',
       at: Time.utc(2024, 6, 3), raw_data: { 'activity_type' => 'DIVNRA' })
    tx(entry_type: :fee, base_currency: 'USD', base_amount: '2', at: Time.utc(2024, 6, 3),
       raw_data: { 'activity_type' => 'FEE' })

    rows = CSV.parse(report(2024))

    assert_includes rows, ['WORKSHEET — INCOME']
    assert_equal(1, rows.count { |row| row[2] == 'DIV' })
    assert_includes rows, ['2024-06-03', 'ZQX', 'DIV', 'Dividend (Anlage KAP)', '100.0', '1.0', '100.0', nil]
    assert_equal(1, rows.count { |row| row[2] == 'DIVNRA' })
    assert_includes rows,
                    ['2024-06-03', 'ZQX', 'DIVNRA', 'Withholding tax (Anlage KAP, line 41)', '15.0', '1.0', '15.0', nil]
    assert_includes rows, [nil, nil, 'Regulatory fees (for information only, not deducted)', nil, nil, nil, '2.0', nil]
  end

  test 'CsvSafe escapes a formula-leading symbol in the disposal worksheet' do
    symbol = '=SUM(A1)'
    clean_share_round_trip(symbol)

    rows = CSV.parse(report(2024))

    assert_includes rows, ['WORKSHEET — DISPOSALS']
    disposal = rows.find { |row| row[0] == "'=SUM(A1)" && row[3] == '2024-09-02' }
    assert_equal 'Share', disposal[1]
    assert_equal '589.0', disposal[11]
    refute rows.flatten.compact.include?(symbol)
  end

  test 'German locale renders the KAP and KAP-INV form labels' do
    clean_share_round_trip
    seed_fx(Date.new(2024, 6, 3), '1.00')
    classify('FNDX', kind: :fund, fund_category: :other_fund)
    tx(entry_type: :other_income, base_currency: 'USD', base_amount: '200', quote_currency: 'FNDX',
       at: Time.utc(2024, 6, 3), raw_data: { 'activity_type' => 'DIV' })

    rows = CSV.parse(I18n.with_locale(:de) { report(2024) })

    assert_includes rows, ['ZUSAMMENFASSUNG — ANLAGE KAP']
    assert_includes rows, ['ZUSAMMENFASSUNG — ANLAGE KAP-INV']
    assert_includes rows, ['Zeile', 'Fondskategorie', 'Position', 'Betrag (EUR)']
    assert_includes rows, ['13', 'Sonstige Fonds', 'Vorabpauschale (vor Teilfreistellung)', '0.0']
  end

  # A fund category with no Zeile mapping has no place to go on the form. Skipping it would drop a
  # whole category out of a signed summary with no banner, which is the failure this report exists
  # to prevent — so it must be loud. The hash is edited directly because the enum has all five today.
  test 'a fund category with no Zeile mapping raises rather than vanishing from the summary' do
    seed_fx(Date.new(2024, 6, 3), '1.00')
    classify('FNDX', kind: :fund, fund_category: :other_fund)
    tx(entry_type: :other_income, base_currency: 'USD', base_amount: '200', quote_currency: 'FNDX',
       at: Time.utc(2024, 6, 3), raw_data: { 'activity_type' => 'DIV' })
    result = Tax::BrokerReport.new(user: @user, year: 2024, exchange: @exchange).result
    result[:kap_inv][:infrastructure_fund] = { distributions: 1.to_d, vorabpauschale: 0.to_d, sale_result: 0.to_d }

    error = assert_raises(ArgumentError) { Tax::BrokerReportCsv.new(result).to_csv }

    assert_includes error.message, 'infrastructure_fund'
  end

  # The lot rows put an ACQUISITION date in the same column as the sale row's disposal date, so the
  # header has to be neutral. A column headed "Disposal date" over an acquisition date is the kind
  # of contradiction that costs someone an hour beside the printed form.
  test 'the disposal date column is headed neutrally because lot rows put an acquisition date in it' do
    clean_share_round_trip

    rows = CSV.parse(report(2024))
    headers = rows.find { |row| row[0] == 'Security' }

    assert_equal 'Date', headers[3]
    assert_equal '2024-09-02', rows.find { |row| row[0] == 'ZQX' }[3]
    assert_equal '2024-03-01', rows.find { |row| row[1] == 'of which: acquisition' }[3]
  end

  # The renderer takes a plain hash, so a warning code added to the engine later arrives here with
  # no translation. It must degrade to a readable line rather than printing the Ruby symbol at a
  # taxpayer. The hash is edited directly because the engine cannot emit an unknown code by design.
  test 'a warning code with no translation renders as text rather than a bare Ruby symbol' do
    clean_share_round_trip
    result = Tax::BrokerReport.new(user: @user, year: 2024, exchange: @exchange).result
    result[:warnings] << { code: :a_code_added_later, symbol: 'ZQX', detail: 'XYZ' }

    csv = Tax::BrokerReportCsv.new(result).to_csv

    assert_includes CSV.parse(csv), ['ZQX', 'Warning a_code_added_later: XYZ']
    refute_includes csv, ':a_code_added_later'
  end

  # A money cell must stay a NUMBER. Formatting one as a String for a tidy two decimals would hand
  # every loss to CsvSafe with a leading '-', which is a formula leader: the cell would be exported
  # as the text "'-500.0" and stop summing in the spreadsheet. Only losses show the bug, so a
  # profitable fixture cannot catch it.
  test 'a loss stays a negative number rather than formula-escaped text, and rounds at the cell' do
    seed_fx(Date.new(2024, 2, 1), '1.00')
    seed_fx(Date.new(2024, 8, 1), '1.10')
    classify('ZQX', kind: :share)
    tx(entry_type: :buy, base_currency: 'ZQX', base_amount: '10', quote_currency: 'USD',
       quote_amount: '2000', at: Time.utc(2024, 2, 1))
    tx(entry_type: :sell, base_currency: 'ZQX', base_amount: '10', quote_currency: 'USD',
       quote_amount: '1500', at: Time.utc(2024, 8, 1))

    csv = report(2024)
    rows = CSV.parse(csv)

    # 1 500 / 1.10 = 1 363.6363... EUR, rounded once at the cell, and 1 363.64 - 2 000 = -636.36.
    disposal = rows.find { |row| row[0] == 'ZQX' && row[3] == '2024-08-01' }
    assert_equal '1363.64', disposal[7]
    assert_equal '-636.36', disposal[11]
    # Z23 takes the loss as a positive magnitude; Z19 keeps it signed.
    assert_equal(['19', 'Foreign investment income', '-636.36'], rows.find { |row| row[0] == '19' })
    assert_equal(['23', 'Losses from the disposal of shares included in line 19', '636.36'], rows.find { |row| row[0] == '23' })
    refute_includes csv, "'-"
  end

  private

  def clean_share_round_trip(symbol = 'ZQX')
    seed_fx(Date.new(2024, 3, 1), '1.10')
    seed_fx(Date.new(2024, 9, 2), '1.00')
    classify(symbol, kind: :share)
    tx(entry_type: :buy, base_currency: symbol, base_amount: '10', quote_currency: 'USD',
       quote_amount: '1000', fee_currency: 'USD', fee_amount: '1', at: Time.utc(2024, 3, 1))
    tx(entry_type: :sell, base_currency: symbol, base_amount: '10', quote_currency: 'USD',
       quote_amount: '1500', fee_currency: 'USD', fee_amount: '1', at: Time.utc(2024, 9, 2))
  end

  def english_disclosures
    [
      'This report is a calculation aid and does not constitute tax advice. Check all amounts before entering them in your tax return.',
      'Currency gains from the interest-bearing USD cash account are income under §20(2) no. 7 EStG ' \
      '(BMF letter dated 14 May 2025, para. 131). They are not included in this report.',
      'US-domiciled ETFs are treated here as other funds under §20(4) InvStG unless evidence is provided ' \
      '(0% Teilfreistellung). Anyone selecting a different fund category is responsible for substantiating ' \
      'the equity participation ratio.',
      "Converted using ECB euro reference rates; on days without a rate, the previous banking day's rate applies.",
      'Interest credited net of foreign withholding tax is reported net on line 19, while the withholding tax ' \
      'is credited on line 41 — the bank does not provide a gross amount for interest. Dividends, by contrast, ' \
      'are grossed up.',
      'Aggregate regulatory fees cannot be attributed to an individual disposal and are therefore not deducted ' \
      'from any amount; they are shown for information only.'
    ]
  end

  def report(year)
    Tax::BrokerReport.new(user: @user, year: year, exchange: @exchange).to_csv
  end

  def classify(symbol, kind:, fund_category: nil)
    FundClassification.create!(user: @user, symbol: symbol, kind: kind, fund_category: fund_category)
  end

  def seed_fx(date, rate)
    FxRate.create!(currency: 'USD', date: date, rate: rate.to_d)
  end

  def tx(entry_type:, base_currency:, base_amount:, at:, quote_currency: nil, quote_amount: nil,
         fee_currency: nil, fee_amount: nil, raw_data: {})
    create(
      :account_transaction,
      user: @user,
      api_key: @api_key,
      exchange: @exchange,
      entry_type: entry_type,
      base_currency: base_currency,
      base_amount: base_amount.to_d,
      quote_currency: quote_currency,
      quote_amount: quote_amount&.to_d,
      fee_currency: fee_currency,
      fee_amount: fee_amount&.to_d,
      transacted_at: at,
      raw_data: raw_data
    )
  end
end
