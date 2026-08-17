require 'test_helper'

class Tax::BrokerReportTest < ActiveSupport::TestCase
  setup do
    @user = create(:user)
    @exchange = create(:alpaca_exchange)
    @api_key = create(:api_key, user: @user, exchange: @exchange)
  end

  test 'share round trip converts each leg separately and reports the KAP share gain' do
    seed_fx(Date.new(2024, 3, 1), '1.10')
    seed_fx(Date.new(2024, 9, 2), '1.00')
    classify('ZQX', kind: :share)
    tx(entry_type: :buy, base_currency: 'ZQX', base_amount: '10', quote_currency: 'USD',
       quote_amount: '1000', fee_currency: 'USD', fee_amount: '1', at: Time.utc(2024, 3, 1))
    tx(entry_type: :sell, base_currency: 'ZQX', base_amount: '10', quote_currency: 'USD',
       quote_amount: '1500', fee_currency: 'USD', fee_amount: '1', at: Time.utc(2024, 9, 2))

    result = report(2024)
    disposal = result[:disposals].sole

    # The acquisition carries its direct fee: (1 000 + 1) / 1.10 = 910 EUR.
    assert_equal '910'.to_d, disposal[:cost_eur]
    # The disposal fee reduces proceeds: (1 500 - 1) / 1.00 = 1 499 EUR.
    assert_equal '1499'.to_d, disposal[:proceeds_eur]
    # Separate-date conversion makes the gain 1 499 - 910 = 589 EUR.
    assert_equal '589'.to_d, disposal[:gain_eur]
    # A share gain is the contained-in-Z19 Z20 amount: 589 EUR.
    assert_equal '589'.to_d, result[:kap][:z20_share_gains]
    # With no other KAP income or loss, the signed saldo is 589 EUR.
    assert_equal '589'.to_d, result[:kap][:z19_saldo]
    # A positive round trip contributes no share-loss magnitude: 0 EUR.
    assert_equal '0'.to_d, result[:kap][:z23_share_losses]
    # Nothing was refused or approximated, so a renderer may present the summary as final.
    assert result[:complete]
    assert_empty result[:refused_symbols]
  end

  test 'losses land in the form bucket selected by instrument kind' do
    seed_fx(Date.new(2024, 2, 1), '1.00')
    seed_fx(Date.new(2024, 8, 1), '1.00')
    classify('ZQX', kind: :share)
    classify('ETN1', kind: :other_security)
    classify('FNDX', kind: :fund, fund_category: :other_fund)

    tx(entry_type: :buy, base_currency: 'ZQX', base_amount: '10', quote_currency: 'USD',
       quote_amount: '2000', at: Time.utc(2024, 2, 1))
    tx(entry_type: :sell, base_currency: 'ZQX', base_amount: '10', quote_currency: 'USD',
       quote_amount: '1500', at: Time.utc(2024, 8, 1))
    tx(entry_type: :buy, base_currency: 'ETN1', base_amount: '10', quote_currency: 'USD',
       quote_amount: '1000', at: Time.utc(2024, 2, 1))
    tx(entry_type: :sell, base_currency: 'ETN1', base_amount: '10', quote_currency: 'USD',
       quote_amount: '700', at: Time.utc(2024, 8, 1))
    tx(entry_type: :buy, base_currency: 'FNDX', base_amount: '10', quote_currency: 'USD',
       quote_amount: '1000', at: Time.utc(2024, 2, 1))
    tx(entry_type: :sell, base_currency: 'FNDX', base_amount: '10', quote_currency: 'USD',
       quote_amount: '600', at: Time.utc(2024, 8, 1))

    result = report(2024)

    # The share loss is entered as a positive magnitude: 2 000 - 1 500 = 500 EUR in Z23.
    assert_equal '500'.to_d, result[:kap][:z23_share_losses]
    # The non-share-security loss is a positive magnitude: 1 000 - 700 = 300 EUR in Z22.
    assert_equal '300'.to_d, result[:kap][:z22_other_losses]
    # Neither non-fund disposal made a gain, so Z20 is 0 EUR.
    assert_equal '0'.to_d, result[:kap][:z20_share_gains]
    # Z19 remains signed and excludes funds: -500 + -300 = -800 EUR.
    assert_equal '-800'.to_d, result[:kap][:z19_saldo]
    # The fund stays wholly in KAP-INV: 600 - 1 000 = -400 EUR.
    assert_equal '-400'.to_d, result[:kap_inv][:other_fund][:sale_result]
  end

  test 'gross dividends and withholding are separated between KAP and KAP-INV' do
    seed_fx(Date.new(2024, 1, 2), '1.00')
    seed_fx(Date.new(2024, 6, 3), '1.25')
    classify('ZQX', kind: :share)
    classify('FNDX', kind: :fund, fund_category: :other_fund)

    tx(entry_type: :buy, base_currency: 'ZQX', base_amount: '10', quote_currency: 'USD',
       quote_amount: '1000', at: Time.utc(2024, 1, 2))
    tx(entry_type: :buy, base_currency: 'FNDX', base_amount: '10', quote_currency: 'USD',
       quote_amount: '1000', at: Time.utc(2024, 1, 2))
    tx(entry_type: :other_income, base_currency: 'USD', base_amount: '100', quote_currency: 'ZQX',
       at: Time.utc(2024, 6, 3), raw_data: { 'activity_type' => 'DIV' })
    tx(entry_type: :withholding_tax, base_currency: 'USD', base_amount: '15', quote_currency: 'ZQX',
       at: Time.utc(2024, 6, 3), raw_data: { 'activity_type' => 'DIVNRA' })
    tx(entry_type: :other_income, base_currency: 'USD', base_amount: '200', quote_currency: 'FNDX',
       at: Time.utc(2024, 6, 3), raw_data: { 'activity_type' => 'DIV' })
    tx(entry_type: :withholding_tax, base_currency: 'USD', base_amount: '30', quote_currency: 'FNDX',
       at: Time.utc(2024, 6, 3), raw_data: { 'activity_type' => 'DIVNRA' })

    result = report(2024)

    # Only the share dividend enters Z19: 100 / 1.25 = 80 EUR.
    assert_equal '80'.to_d, result[:kap][:z19_saldo]
    # Both securities' withholding is creditable: 15 / 1.25 + 30 / 1.25 = 12 + 24 = 36 EUR.
    assert_equal '36'.to_d, result[:kap][:z41_withholding_tax]
    # The fund distribution bypasses Z19: 200 / 1.25 = 160 EUR in KAP-INV.
    assert_equal '160'.to_d, result[:kap_inv][:other_fund][:distributions]
  end

  test 'an unclassified symbol blocks summaries but preserves worksheet evidence' do
    seed_fx(Date.new(2024, 3, 1), '1.00')
    tx(entry_type: :buy, base_currency: 'UNK', base_amount: '10', quote_currency: 'USD',
       quote_amount: '1000', at: Time.utc(2024, 3, 1))
    tx(entry_type: :sell, base_currency: 'UNK', base_amount: '10', quote_currency: 'USD',
       quote_amount: '1500', at: Time.utc(2024, 3, 1, 12))

    result = report(2024)

    assert_not result[:summaries_available]
    assert_nil result[:kap]
    assert_nil result[:kap_inv]
    assert_equal ['UNK'], result[:unclassified_symbols]
    assert_predicate result[:disposals], :present?
    # The to-do row Task 13 renders: nothing to propose and nothing decided yet.
    todo = result[:symbols].sole
    assert_equal 'UNK', todo[:symbol]
    assert_not todo[:classified]
    assert_not todo[:persisted]
    assert_nil todo[:kind]
    assert_nil todo[:fund_category]
  end

  test 'classification rows use the report resolver and exclude Alpaca crypto' do
    create(:asset, symbol: 'CRYP', category: 'Cryptocurrency')
    create(:asset, symbol: 'ETFX', category: 'Stock', instrument_type: 'etf')
    create(:asset, symbol: 'OVR', category: 'Stock', instrument_type: 'etf')
    classify('OVR', kind: :share)
    %w[CRYP ETFX OVR].each do |symbol|
      tx(entry_type: :buy, base_currency: symbol, base_amount: '1', quote_currency: 'USD',
         quote_amount: '100', at: Time.utc(2024, 3, 1))
    end

    rows = Tax::BrokerReport.new(user: @user, year: 2024, exchange: @exchange).classification_rows

    refute_includes rows.pluck(:symbol), 'CRYP'
    proposal = rows.find { |row| row[:symbol] == 'ETFX' }
    assert_equal({ classified: true, persisted: false, kind: :fund, fund_category: :other_fund },
                 proposal.slice(:classified, :persisted, :kind, :fund_category))
    persisted = rows.find { |row| row[:symbol] == 'OVR' }
    assert_equal({ classified: true, persisted: true, kind: :share, fund_category: nil },
                 persisted.slice(:classified, :persisted, :kind, :fund_category))
  end

  # The panel is the only place a user is told a security's figures will be withheld, and the two
  # refusals that need no price and no FX rate are cheap enough to compute for a modal.
  test 'classification rows carry the refusals that cost nothing to compute' do
    create(:asset, symbol: 'FNDX', category: 'Stock', instrument_type: 'etf')
    classify('ZQX', kind: :share)
    tx(entry_type: :buy, base_currency: 'ZQX', base_amount: '10', quote_currency: 'USD',
       quote_amount: '1000', at: Time.utc(2024, 2, 1))
    tx(entry_type: :unsupported_activity, base_currency: 'ZQX', base_amount: '1',
       at: Time.utc(2024, 5, 1), raw_data: { 'activity_type' => 'OPEXC' })
    tx(entry_type: :buy, base_currency: 'FNDX', base_amount: '10', quote_currency: 'USD',
       quote_amount: '1000', at: Time.utc(2017, 6, 1))

    rows = Tax::BrokerReport.new(user: @user, year: 2024, exchange: @exchange).classification_rows

    refused = rows.find { |row| row[:symbol] == 'ZQX' }
    assert refused[:refused]
    assert_equal [:unsupported_activity], refused[:refusal_reasons]
    # §56 InvStG lots are found from the proposed classification alone — no price, no FX rate.
    old_lot = rows.find { |row| row[:symbol] == 'FNDX' }
    assert old_lot[:refused]
    assert_equal [:pre_2018_fund_lot], old_lot[:refusal_reasons]
  end

  test 'report year declares prior-year VAP and deducts it from a later fund sale' do
    [
      Date.new(2023, 12, 29), Date.new(2024, 2, 1), Date.new(2024, 4, 15),
      Date.new(2024, 6, 20), Date.new(2024, 12, 31), Date.new(2025, 3, 10)
    ].each { |date| seed_fx(date, '1.00') }
    classify('FNDX', kind: :fund, fund_category: :other_fund)
    seed_stock_price('FNDX', Date.new(2023, 12, 29), '10.00')
    seed_stock_price('FNDX', Date.new(2024, 12, 31), '12.00')

    tx(entry_type: :buy, base_currency: 'FNDX', base_amount: '100', quote_currency: 'USD',
       quote_amount: '500', at: Time.utc(2024, 2, 1))
    tx(entry_type: :sell, base_currency: 'FNDX', base_amount: '100', quote_currency: 'USD',
       quote_amount: '700', at: Time.utc(2024, 6, 20))
    tx(entry_type: :buy, base_currency: 'FNDX', base_amount: '100', quote_currency: 'USD',
       quote_amount: '1000', at: Time.utc(2024, 4, 15))
    tx(entry_type: :sell, base_currency: 'FNDX', base_amount: '100', quote_currency: 'USD',
       quote_amount: '1500', at: Time.utc(2025, 3, 10))

    result = report(2025)

    # April's 9/12 reduction gives 1 000 * 0.7 * 0.0229 * 9 / 12 = 12.0225 EUR.
    assert_equal '12.0225'.to_d, result[:kap_inv][:other_fund][:vorabpauschale]
    # The gross §19 deduction leaves 1 500 - 1 000 - 12.0225 = 487.9775 EUR.
    assert_equal '487.9775'.to_d, result[:kap_inv][:other_fund][:sale_result]
    assert_equal 1, result[:disposals].size
    # The sole report-year sale consumes the same accumulated 12.0225 EUR VAP.
    assert_equal '12.0225'.to_d, result[:disposals].sole[:vorabpauschale_deducted_eur]
  end

  test 'options activity refuses its symbol without hiding the disposal' do
    seed_fx(Date.new(2024, 2, 1), '1.00')
    seed_fx(Date.new(2024, 8, 1), '1.00')
    classify('ZQX', kind: :share)
    tx(entry_type: :buy, base_currency: 'ZQX', base_amount: '10', quote_currency: 'USD',
       quote_amount: '1000', at: Time.utc(2024, 2, 1))
    tx(entry_type: :unsupported_activity, base_currency: 'ZQX', base_amount: '1', at: Time.utc(2024, 5, 1),
       raw_data: { 'activity_type' => 'OPEXC' })
    tx(entry_type: :sell, base_currency: 'ZQX', base_amount: '10', quote_currency: 'USD',
       quote_amount: '1500', at: Time.utc(2024, 8, 1))

    result = report(2024)
    symbol = result[:symbols].find { |entry| entry[:symbol] == 'ZQX' }

    assert(result[:warnings].any? { |warning| warning[:code] == :unsupported_activity && warning[:symbol] == 'ZQX' })
    assert symbol[:refused]
    assert_includes symbol[:refusal_reasons], :unsupported_activity
    # Refusal removes the otherwise reportable 1 500 - 1 000 = 500 EUR share gain from Z20.
    assert_equal '0'.to_d, result[:kap][:z20_share_gains]
    # The same refused 500 EUR cannot leak into the Z19 saldo.
    assert_equal '0'.to_d, result[:kap][:z19_saldo]
    assert result[:disposals].sole[:refused]
    # An all-zero KAP is not a finished one: `summaries_available` only covers classification, so a
    # renderer keying its banner on that alone would present this as complete.
    assert result[:summaries_available]
    assert_not result[:complete]
    assert_equal ['ZQX'], result[:refused_symbols]
  end

  test 'missing FX refuses only the affected symbol' do
    seed_fx(Date.new(2024, 2, 1), '1.00')
    seed_fx(Date.new(2024, 8, 1), '1.00')
    classify('ZQX', kind: :share)
    classify('ETN1', kind: :other_security)
    tx(entry_type: :buy, base_currency: 'ZQX', base_amount: '10', quote_currency: 'USD',
       quote_amount: '1000', at: Time.utc(2024, 2, 1))
    tx(entry_type: :sell, base_currency: 'ZQX', base_amount: '10', quote_currency: 'USD',
       quote_amount: '1500', at: Time.utc(2024, 8, 1))
    tx(entry_type: :buy, base_currency: 'ETN1', base_amount: '10', quote_currency: 'USD',
       quote_amount: '1000', at: Time.utc(2024, 2, 1))
    tx(entry_type: :sell, base_currency: 'ETN1', base_amount: '10', quote_currency: 'USD',
       quote_amount: '700', at: Time.utc(2024, 11, 20))

    result = report(2024)
    etn = result[:symbols].find { |entry| entry[:symbol] == 'ETN1' }

    # Per-symbol refusal preserves the unaffected share gain: 1 500 - 1 000 = 500 EUR.
    assert_equal '500'.to_d, result[:kap][:z20_share_gains]
    # The unconvertible ETN loss contributes 0 EUR instead of a guessed Z22 figure.
    assert_equal '0'.to_d, result[:kap][:z22_other_losses]
    assert etn[:refused]
    assert_includes etn[:refusal_reasons], :missing_fx
    assert_predicate result[:warnings], :present?
  end

  test 'DIVFT is grossed up for income while remaining creditable withholding' do
    seed_fx(Date.new(2024, 1, 2), '1.00')
    seed_fx(Date.new(2024, 6, 3), '1.00')
    classify('ZQX', kind: :share)
    tx(entry_type: :buy, base_currency: 'ZQX', base_amount: '10', quote_currency: 'USD',
       quote_amount: '1000', at: Time.utc(2024, 1, 2))
    tx(entry_type: :other_income, base_currency: 'USD', base_amount: '85', quote_currency: 'ZQX',
       at: Time.utc(2024, 6, 3), raw_data: { 'activity_type' => 'DIV' })
    tx(entry_type: :withholding_tax, base_currency: 'USD', base_amount: '15', quote_currency: 'ZQX',
       at: Time.utc(2024, 6, 3), raw_data: { 'activity_type' => 'DIVFT' })

    result = report(2024)

    # DIVFT proves the DIV is net, so gross income is (85 + 15) / 1.00 = 100 EUR.
    assert_equal '100'.to_d, result[:kap][:z19_saldo]
    # The gross-up does not erase the credit: 15 / 1.00 = 15 EUR in Z41.
    assert_equal '15'.to_d, result[:kap][:z41_withholding_tax]
  end

  test 'a mid-year split leaves lot-level VAP unchanged' do
    [
      Date.new(2023, 12, 29), Date.new(2024, 1, 5), Date.new(2024, 7, 1), Date.new(2024, 12, 31)
    ].each { |date| seed_fx(date, '1.00') }
    classify('FNDX', kind: :fund, fund_category: :other_fund)
    classify('FNDY', kind: :fund, fund_category: :other_fund)
    seed_stock_price('FNDX', Date.new(2023, 12, 29), '10.00')
    seed_stock_price('FNDX', Date.new(2024, 12, 31), '6.00')
    seed_stock_price('FNDY', Date.new(2023, 12, 29), '10.00')
    seed_stock_price('FNDY', Date.new(2024, 12, 31), '12.00')

    tx(entry_type: :buy, base_currency: 'FNDX', base_amount: '100', quote_currency: 'USD',
       quote_amount: '1000', at: Time.utc(2024, 1, 5))
    tx(entry_type: :adjustment, base_currency: 'FNDX', base_amount: '100', at: Time.utc(2024, 7, 1),
       raw_data: { 'activity_type' => 'SSP' })
    tx(entry_type: :buy, base_currency: 'FNDY', base_amount: '100', quote_currency: 'USD',
       quote_amount: '1000', at: Time.utc(2024, 1, 5))

    result = report(2025)

    # Both lots retain 1 000 EUR start and 1 200 EUR end values, so 16.03 + 16.03 = 32.06 EUR.
    assert_equal '32.06'.to_d, result[:kap_inv][:other_fund][:vorabpauschale]
  end

  test 'a post-split acquisition is restated into year-start share terms for VAP' do
    [
      Date.new(2023, 12, 29), Date.new(2024, 1, 5), Date.new(2024, 3, 1),
      Date.new(2024, 7, 10), Date.new(2024, 12, 31)
    ].each { |date| seed_fx(date, '1.00') }
    classify('FNDX', kind: :fund, fund_category: :other_fund)
    seed_stock_price('FNDX', Date.new(2023, 12, 29), '10.00')
    seed_stock_price('FNDX', Date.new(2024, 12, 31), '6.00')

    tx(entry_type: :buy, base_currency: 'FNDX', base_amount: '100', quote_currency: 'USD',
       quote_amount: '1000', at: Time.utc(2024, 1, 5))
    tx(entry_type: :adjustment, base_currency: 'FNDX', base_amount: '100', at: Time.utc(2024, 3, 1),
       raw_data: { 'activity_type' => 'SSP' })
    tx(entry_type: :buy, base_currency: 'FNDX', base_amount: '50', quote_currency: 'USD',
       quote_amount: '300', at: Time.utc(2024, 7, 10))

    result = report(2025)

    # Lot P contributes 16.03 and lot Q contributes 250 * 0.7 * 0.0229 * 6 / 12 = 2.00375,
    # totaling 18.03375 EUR. Using Q's post-split 50 units would instead make its negative
    # Mehrbetrag floor VAP at 0, which is the plausible year-start-restatement error guarded here.
    assert_equal '18.03375'.to_d, result[:kap_inv][:other_fund][:vorabpauschale]
  end

  test 'an uncovered disposal refuses the zero-basis tail' do
    seed_fx(Date.new(2024, 2, 1), '1.00')
    seed_fx(Date.new(2024, 8, 1), '1.00')
    classify('ZQX', kind: :share)
    tx(entry_type: :buy, base_currency: 'ZQX', base_amount: '4', quote_currency: 'USD',
       quote_amount: '400', at: Time.utc(2024, 2, 1))
    tx(entry_type: :sell, base_currency: 'ZQX', base_amount: '10', quote_currency: 'USD',
       quote_amount: '1500', at: Time.utc(2024, 8, 1))

    result = report(2024)
    symbol = result[:symbols].find { |entry| entry[:symbol] == 'ZQX' }
    disposal = result[:disposals].sole

    assert symbol[:refused]
    assert_includes symbol[:refusal_reasons], :uncovered_disposal
    # Refusal prevents the fabricated 1 500 - 400 = 1 100 EUR from entering Z19.
    assert_equal '0'.to_d, result[:kap][:z19_saldo]
    # The same fabricated 1 100 EUR cannot appear in the share-gain breakout.
    assert_equal '0'.to_d, result[:kap][:z20_share_gains]
    assert disposal[:refused]
    assert(disposal[:matches].any? { |match| match[:uncovered] })
  end

  test 'a pre-2018 fund lot refuses the unimplemented transition calculation' do
    seed_fx(Date.new(2017, 6, 1), '1.00')
    seed_fx(Date.new(2024, 8, 1), '1.00')
    classify('FNDX', kind: :fund, fund_category: :other_fund)
    seed_stock_price('FNDX', Date.new(2023, 12, 29), '10.00')
    tx(entry_type: :buy, base_currency: 'FNDX', base_amount: '10', quote_currency: 'USD',
       quote_amount: '1000', at: Time.utc(2017, 6, 1))
    tx(entry_type: :sell, base_currency: 'FNDX', base_amount: '10', quote_currency: 'USD',
       quote_amount: '1500', at: Time.utc(2024, 8, 1))
    tx(entry_type: :withholding_tax, base_currency: 'USD', base_amount: '30', quote_currency: 'FNDX',
       at: Time.utc(2024, 8, 1), raw_data: { 'activity_type' => 'DIVNRA' })

    result = report(2024)
    symbol = result[:symbols].find { |entry| entry[:symbol] == 'FNDX' }

    assert symbol[:refused]
    assert_includes symbol[:refusal_reasons], :pre_2018_fund_lot
    # A refused fund contributes no speculative 1 500 - 1 000 = 500 EUR category, so the key is absent.
    refute_includes result[:kap_inv].keys, :other_fund
    # The §56 gap is about basis, not about whether tax was withheld, so the credit survives it:
    # 30 / 1.00 = 30 EUR. Dropping it would cost the taxpayer money invisibly.
    assert_equal '30'.to_d, result[:kap][:z41_withholding_tax]
    assert(result[:warnings].any? do |warning|
      warning[:code] == :withholding_credited_for_refused_symbol && warning[:detail] == ['FNDX']
    end)
  end

  test 'an interest reversal is subtracted from the KAP saldo rather than added to it' do
    seed_fx(Date.new(2024, 5, 2), '1.00')
    seed_fx(Date.new(2024, 10, 7), '1.00')
    tx(entry_type: :other_income, base_currency: 'USD', base_amount: '50', at: Time.utc(2024, 5, 2),
       raw_data: { 'activity_type' => 'INT', 'net_amount' => '50' })
    # INT routes through the same normaliser as DIV, so a correction arrives sign-stripped too.
    tx(entry_type: :other_income, base_currency: 'USD', base_amount: '20', at: Time.utc(2024, 10, 7),
       raw_data: { 'activity_type' => 'INT', 'net_amount' => '-20' })

    result = report(2024)

    # The taxpayer received 50 - 20 = 30 EUR; the absolute value would declare 70.
    assert_equal '30'.to_d, result[:kap][:z19_saldo]
  end

  test 'a computation year with no published Basiszins refuses the symbol' do
    [Date.new(2026, 12, 31), Date.new(2027, 1, 5), Date.new(2027, 12, 31)].each { |date| seed_fx(date, '1.00') }
    classify('FNDX', kind: :fund, fund_category: :other_fund)
    seed_stock_price('FNDX', Date.new(2026, 12, 31), '10.00')
    seed_stock_price('FNDX', Date.new(2027, 12, 31), '12.00')
    tx(entry_type: :buy, base_currency: 'FNDX', base_amount: '100', quote_currency: 'USD',
       quote_amount: '1000', at: Time.utc(2027, 1, 5))

    result = report(2028)

    # The 2027 rate is not published yet, and a silent 0 would be indistinguishable from a
    # legitimately zero Vorabpauschale.
    assert_includes result[:symbols].sole[:refusal_reasons], :missing_basiszins
    assert_empty result[:kap_inv]
    assert_not result[:complete]
  end

  test 'a stock ticker colliding with a crypto asset is never silently dropped' do
    seed_fx(Date.new(2024, 2, 1), '1.00')
    seed_fx(Date.new(2024, 8, 1), '1.00')
    # `symbol` is not unique on assets, so any CoinGecko coin sharing a ticker used to shadow the
    # real holding and remove it from the report with no warning and no refusal.
    create(:asset, symbol: 'ZQX', external_id: 'zqx-coin', category: 'Cryptocurrency')
    classify('ZQX', kind: :share)
    tx(entry_type: :buy, base_currency: 'ZQX', base_amount: '10', quote_currency: 'USD',
       quote_amount: '1000', at: Time.utc(2024, 2, 1))
    tx(entry_type: :sell, base_currency: 'ZQX', base_amount: '10', quote_currency: 'USD',
       quote_amount: '1500', at: Time.utc(2024, 8, 1))

    result = report(2024)

    # The user's own classification outranks the catalogue: 1 500 - 1 000 = 500 EUR still declared.
    assert_equal '500'.to_d, result[:kap][:z20_share_gains]
    assert_equal ['ZQX'], result[:symbols].map { |entry| entry[:symbol] }.sort
    # A single category is not a collision the user needs to look at.
    assert result[:complete]
  end

  test 'a ticker spanning two asset categories is reported and flagged for the user' do
    seed_fx(Date.new(2024, 2, 1), '1.00')
    seed_fx(Date.new(2024, 8, 1), '1.00')
    create(:asset, symbol: 'ZQX', external_id: 'zqx-coin', category: 'Cryptocurrency')
    create(:asset, symbol: 'ZQX', external_id: 'zqx-stock', category: 'Stock', instrument_type: 'stock')
    tx(entry_type: :buy, base_currency: 'ZQX', base_amount: '10', quote_currency: 'USD',
       quote_amount: '1000', at: Time.utc(2024, 2, 1))
    tx(entry_type: :sell, base_currency: 'ZQX', base_amount: '10', quote_currency: 'USD',
       quote_amount: '1500', at: Time.utc(2024, 8, 1))

    result = report(2024)

    # The stock row resolves the ticker, so the 500 EUR gain is declared and proposed as a share.
    assert_equal '500'.to_d, result[:kap][:z20_share_gains]
    assert_equal :share, result[:symbols].sole[:kind]
    assert(result[:warnings].any? do |warning|
      warning[:code] == :ambiguous_symbol && warning[:symbol] == 'ZQX' &&
        warning[:detail] == %w[Cryptocurrency Stock]
    end)
    assert_not result[:complete]
  end

  test 'a dividend reversal is subtracted from the KAP saldo rather than added to it' do
    seed_fx(Date.new(2024, 6, 3), '1.00')
    seed_fx(Date.new(2024, 9, 4), '1.00')
    classify('ZQX', kind: :share)
    tx(entry_type: :other_income, base_currency: 'USD', base_amount: '100', quote_currency: 'ZQX',
       at: Time.utc(2024, 6, 3), raw_data: { 'activity_type' => 'DIV', 'net_amount' => '100' })
    # Alpaca books a correction as a negative net_amount, but the ledger stores base_amount absolute.
    tx(entry_type: :other_income, base_currency: 'USD', base_amount: '40', quote_currency: 'ZQX',
       at: Time.utc(2024, 9, 4), raw_data: { 'activity_type' => 'DIV', 'net_amount' => '-40' })

    result = report(2024)

    # Reading the signed net_amount gives (100 - 40) / 1.00 = 60 EUR; the absolute value would
    # have declared 140 EUR of income the user never received.
    assert_equal '60'.to_d, result[:kap][:z19_saldo]
    assert_equal '-40'.to_d, result[:income].last[:eur_amount]
  end

  test 'return of capital beyond basis becomes income in the instrument-specific bucket' do
    seed_fx(Date.new(2024, 2, 1), '1.00')
    seed_fx(Date.new(2024, 6, 3), '1.00')
    classify('ZQX', kind: :share)
    classify('FNDX', kind: :fund, fund_category: :other_fund)
    tx(entry_type: :buy, base_currency: 'ZQX', base_amount: '10', quote_currency: 'USD',
       quote_amount: '1000', at: Time.utc(2024, 2, 1))
    tx(entry_type: :return_of_capital, base_currency: 'ZQX', base_amount: '10', quote_currency: 'USD',
       quote_amount: '1500', at: Time.utc(2024, 6, 3),
       raw_data: { 'activity_type' => 'DIVROC', 'per_share_amount' => '150' })
    tx(entry_type: :buy, base_currency: 'FNDX', base_amount: '10', quote_currency: 'USD',
       quote_amount: '1000', at: Time.utc(2024, 2, 1))
    tx(entry_type: :return_of_capital, base_currency: 'FNDX', base_amount: '10', quote_currency: 'USD',
       quote_amount: '1200', at: Time.utc(2024, 6, 3),
       raw_data: { 'activity_type' => 'DIVROC', 'per_share_amount' => '120' })

    result = report(2024)

    # The share excess joins Z19: 10 * 150 - 1 000 = 500 EUR.
    assert_equal '500'.to_d, result[:kap][:z19_saldo]
    # The fund excess stays in KAP-INV: 10 * 120 - 1 000 = 200 EUR.
    assert_equal '200'.to_d, result[:kap_inv][:other_fund][:distributions]
  end

  test 'interest joins the KAP saldo while a regulatory fee is disclosed but never deducted' do
    seed_fx(Date.new(2024, 2, 1), '1.00')
    seed_fx(Date.new(2024, 5, 2), '1.00')
    seed_fx(Date.new(2024, 8, 1), '1.00')
    classify('ZQX', kind: :share)
    tx(entry_type: :buy, base_currency: 'ZQX', base_amount: '10', quote_currency: 'USD',
       quote_amount: '1000', at: Time.utc(2024, 2, 1))
    tx(entry_type: :sell, base_currency: 'ZQX', base_amount: '10', quote_currency: 'USD',
       quote_amount: '1500', at: Time.utc(2024, 8, 1))
    tx(entry_type: :other_income, base_currency: 'USD', base_amount: '50', at: Time.utc(2024, 5, 2),
       raw_data: { 'activity_type' => 'INT' })
    tx(entry_type: :fee, base_currency: 'USD', base_amount: '20', at: Time.utc(2024, 5, 2),
       raw_data: { 'activity_type' => 'FEE' })

    result = report(2024)

    # Interest carries no security symbol, so it enters Z19 alongside the 500 EUR share gain:
    # (1 500 - 1 000) + 50 = 550 EUR.
    assert_equal '550'.to_d, result[:kap][:z19_saldo]
    # The daily regulatory aggregate is unattributable, so the share gain stays a full 500 EUR.
    assert_equal '500'.to_d, result[:kap][:z20_share_gains]
    # It is disclosed on its own line instead: 20 / 1.00 = 20 EUR.
    assert_equal '20'.to_d, result[:fee_total_eur]
  end

  test 'stock price range slices Alpaca bars and persists namespaced USD closes' do
    seed_fx(Date.new(2024, 6, 3), '1.00')
    base_asset = create(:asset, symbol: 'FNDX', category: 'Stock')
    quote_asset = create(:asset, :usd)
    ticker = create(:ticker, exchange: @exchange, base_asset: base_asset, quote_asset: quote_asset,
                             base: 'FNDX', quote: 'USD', ticker: 'FNDXUSD')
    from = Date.new(2024, 6, 3)
    to = Date.new(2024, 6, 4)
    candles = [
      candle(Time.utc(2024, 6, 3), close: '12.25'),
      candle(Time.utc(2024, 6, 4), close: '13.50'),
      candle(Time.utc(2024, 6, 5), close: '99.00')
    ]
    @exchange.expects(:get_candles)
             .with(ticker: ticker, start_at: from.to_time(:utc), timeframe: 1.day)
             .returns(Result::Success.new(candles))

    prices = Tax::PriceService.new.stock_price_range(
      exchange: @exchange, api_key: @api_key, symbol: 'FNDX', from: from, to: to
    )

    # At a 1.00 USD-per-EUR rate, close / FX keeps 12.25 and 13.50 EUR unchanged.
    assert_equal({ Date.new(2024, 6, 3) => '12.25'.to_d, Date.new(2024, 6, 4) => '13.50'.to_d }, prices)
    stored = HistoricalPrice.where(asset: 'stock:FNDX', currency: 'USD').order(:date).pluck(:date, :price).to_h
    # Persistence retains the two in-window USD closes, 12.25 and 13.50, under the collision-safe namespace.
    assert_equal({ Date.new(2024, 6, 3) => '12.25'.to_d, Date.new(2024, 6, 4) => '13.50'.to_d }, stored)
    assert_nil HistoricalPrice.find_by(asset: 'stock:FNDX', currency: 'USD', date: Date.new(2024, 6, 5))
    assert_nil HistoricalPrice.find_by(asset: 'FNDX', currency: 'USD')
  end

  test 'a partially stored price window is refetched instead of settling on an early price' do
    seed_fx(Date.new(2024, 12, 23), '1.00')
    seed_fx(Date.new(2024, 12, 31), '1.00')
    # A truncated earlier fetch left only the start of the window behind.
    seed_stock_price('FNDX', Date.new(2024, 12, 23), '9.00')
    base_asset = create(:asset, symbol: 'FNDX', category: 'Stock')
    create(:ticker, exchange: @exchange, base_asset: base_asset, quote_asset: create(:asset, :usd),
                    base: 'FNDX', quote: 'USD', ticker: 'FNDXUSD')
    @exchange.expects(:get_candles).returns(Result::Success.new([candle(Time.utc(2024, 12, 31), close: '12.00')]))

    prices = Tax::PriceService.new.stock_price_range(
      exchange: @exchange, api_key: @api_key, symbol: 'FNDX',
      from: Date.new(2024, 12, 22), to: Date.new(2024, 12, 31)
    )

    # Gating the refetch on "any row at all" would leave the window ending on 23 December, and a
    # Vorabpauschale would take 9.00 EUR as the year-end value instead of 12.00.
    assert_equal '12.00'.to_d, prices[Date.new(2024, 12, 31)]
  end

  test 'stock price range returns empty when a delisted symbol has no ticker' do
    prices = Tax::PriceService.new.stock_price_range(
      exchange: @exchange, api_key: @api_key, symbol: 'UNK',
      from: Date.new(2024, 6, 3), to: Date.new(2024, 6, 4)
    )

    assert_equal({}, prices)
  end

  private

  def report(year)
    Tax::BrokerReport.new(user: @user, year: year, exchange: @exchange).result
  end

  def classify(symbol, kind:, fund_category: nil)
    FundClassification.create!(user: @user, symbol: symbol, kind: kind, fund_category: fund_category)
  end

  def seed_fx(date, rate)
    FxRate.create!(currency: 'USD', date: date, rate: rate.to_d)
  end

  def seed_stock_price(symbol, date, price)
    HistoricalPrice.create!(asset: "stock:#{symbol}", currency: 'USD', date: date, price: price.to_d)
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

  def candle(time, close:)
    [time, '10'.to_d, '14'.to_d, '9'.to_d, close.to_d, '100'.to_d]
  end
end
