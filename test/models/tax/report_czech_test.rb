require 'test_helper'

class Tax::ReportCzechTest < ActiveSupport::TestCase
  setup do
    @report = Tax::Report.allocate
    @report.instance_variable_set(:@jurisdiction, Tax::Jurisdictions.for('CZ'))
  end

  # --- Time test ---

  test 'time test exempts disposals held over 3 years' do
    disposals = [
      { holding_days: 1096, proceeds: 200_000.to_d, gain_loss: 50_000.to_d }
    ]

    @report.send(:apply_czech_exemptions, disposals)

    assert_equal true, disposals.first[:tax_exempt]
    assert_equal 'časový test', disposals.first[:exempt_reason]
  end

  test 'time test does not exempt disposals held under 3 years' do
    disposals = [
      { holding_days: 1000, proceeds: 200_000.to_d, gain_loss: 50_000.to_d }
    ]

    @report.send(:apply_czech_exemptions, disposals)

    assert_equal false, disposals.first[:tax_exempt]
    assert_nil disposals.first[:exempt_reason]
  end

  # --- Value test ---

  test 'value test exempts all disposals when total proceeds under 100k CZK' do
    disposals = [
      { holding_days: 100, proceeds: 40_000.to_d, gain_loss: 5_000.to_d },
      { holding_days: 200, proceeds: 50_000.to_d, gain_loss: 10_000.to_d }
    ]

    @report.send(:apply_czech_exemptions, disposals)

    disposals.each do |d|
      assert_equal true, d[:tax_exempt]
      assert_equal 'hodnotový test', d[:exempt_reason]
    end
  end

  test 'value test overrides time test when proceeds under 100k' do
    disposals = [
      { holding_days: 100, proceeds: 80_000.to_d, gain_loss: 20_000.to_d }
    ]

    @report.send(:apply_czech_exemptions, disposals)

    assert_equal true, disposals.first[:tax_exempt]
    assert_equal 'hodnotový test', disposals.first[:exempt_reason]
  end

  test 'value test not applied when total proceeds exceed 100k CZK' do
    disposals = [
      { holding_days: 100, proceeds: 60_000.to_d, gain_loss: 10_000.to_d },
      { holding_days: 200, proceeds: 50_000.to_d, gain_loss: 5_000.to_d }
    ]

    @report.send(:apply_czech_exemptions, disposals)

    disposals.each do |d|
      assert_equal false, d[:tax_exempt]
      assert_nil d[:exempt_reason]
    end
  end

  # --- Mixed scenarios ---

  test 'mixed: only long-held disposals exempt when proceeds exceed 100k' do
    disposals = [
      { holding_days: 1096, proceeds: 80_000.to_d, gain_loss: 30_000.to_d },
      { holding_days: 200, proceeds: 40_000.to_d, gain_loss: 5_000.to_d }
    ]

    @report.send(:apply_czech_exemptions, disposals)

    assert_equal true, disposals[0][:tax_exempt]
    assert_equal 'časový test', disposals[0][:exempt_reason]
    assert_equal false, disposals[1][:tax_exempt]
    assert_nil disposals[1][:exempt_reason]
  end

  # --- Summary ---

  test 'czech summary shows correct totals' do
    disposals = [
      { proceeds: 200_000.to_d, cost_basis: 150_000.to_d, gain_loss: 50_000.to_d,
        tax_exempt: true, exempt_reason: 'časový test' },
      { proceeds: 100_000.to_d, cost_basis: 80_000.to_d, gain_loss: 20_000.to_d,
        tax_exempt: false, exempt_reason: nil }
    ]

    csv_rows = []
    csv = Object.new
    csv.define_singleton_method(:<<) { |row| csv_rows << row }

    I18n.with_locale(:cs) do
      @report.send(:append_czech_summary, csv, disposals)
    end

    # Total proceeds
    assert_equal 300_000.0, csv_rows[1][1]
    # Total costs
    assert_equal 230_000.0, csv_rows[2][1]
    # Total gain
    assert_equal 70_000.0, csv_rows[3][1]
    # Exempt (time test) — only the 50k gain
    assert_equal 50_000.0, csv_rows[4][1]
    # Value test — Ne
    assert_equal 'Ne', csv_rows[5][1]
    # Taxable gain — 70k - 50k = 20k
    assert_equal 20_000.0, csv_rows[6][1]
  end

  test 'czech summary shows value test as Ano when applicable' do
    disposals = [
      { proceeds: 50_000.to_d, cost_basis: 40_000.to_d, gain_loss: 10_000.to_d,
        tax_exempt: true, exempt_reason: 'hodnotový test' }
    ]

    csv_rows = []
    csv = Object.new
    csv.define_singleton_method(:<<) { |row| csv_rows << row }

    I18n.with_locale(:cs) do
      @report.send(:append_czech_summary, csv, disposals)
    end

    # Value test — Ano
    assert_equal 'Ano', csv_rows[5][1]
    # Taxable gain — 0 (all exempt)
    assert_equal 0, csv_rows[6][1]
  end
end
