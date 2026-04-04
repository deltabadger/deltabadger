require 'test_helper'

class Tax::ReportSlovakiaTest < ActiveSupport::TestCase
  setup do
    @report = Tax::Report.allocate
    @report.instance_variable_set(:@jurisdiction, Tax::Jurisdictions.for('SK'))
  end

  test 'long-term holding gets 7% rate' do
    disposals = [
      { holding_days: 400, gain_loss: 10_000.to_d }
    ]

    @report.send(:apply_short_long_term, disposals)
    @report.send(:apply_holding_tax_rate, disposals)

    assert_equal 'long', disposals.first[:term]
    assert_equal '7%', disposals.first[:tax_rate]
  end

  test 'short-term holding gets 19% rate' do
    disposals = [
      { holding_days: 200, gain_loss: 10_000.to_d }
    ]

    @report.send(:apply_short_long_term, disposals)
    @report.send(:apply_holding_tax_rate, disposals)

    assert_equal 'short', disposals.first[:term]
    assert_equal '19%', disposals.first[:tax_rate]
  end

  test 'mixed disposals get correct rates' do
    disposals = [
      { holding_days: 400, gain_loss: 10_000.to_d },
      { holding_days: 100, gain_loss: 5_000.to_d },
      { holding_days: 366, gain_loss: 3_000.to_d }
    ]

    @report.send(:apply_short_long_term, disposals)
    @report.send(:apply_holding_tax_rate, disposals)

    assert_equal '7%', disposals[0][:tax_rate]
    assert_equal '19%', disposals[1][:tax_rate]
    assert_equal '7%', disposals[2][:tax_rate]
  end

  test 'term values are localized in Slovak' do
    disposals = [
      { holding_days: 400, gain_loss: 10_000.to_d },
      { holding_days: 100, gain_loss: 5_000.to_d }
    ]

    @report.send(:apply_short_long_term, disposals)

    I18n.with_locale(:sk) do
      long_label = I18n.t("tax_report.values.term_#{disposals[0][:term]}", default: disposals[0][:term])
      short_label = I18n.t("tax_report.values.term_#{disposals[1][:term]}", default: disposals[1][:term])

      assert_equal 'Dlhodobý', long_label
      assert_equal 'Krátkodobý', short_label
    end
  end
end
