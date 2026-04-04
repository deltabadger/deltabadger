require 'test_helper'

class Tax::ReportDenmarkTest < ActiveSupport::TestCase
  setup do
    @report = Tax::Report.allocate
    @report.instance_variable_set(:@jurisdiction, Tax::Jurisdictions.for('DK'))
  end

  # --- Wash-sale tests ---

  test 'wash sale triggered when same asset bought between acquisition and sale' do
    enriched = [
      { entry_type: 'buy', base_currency: 'BTC', base_amount: 1.to_d, transacted_at: Time.utc(2024, 1, 1) },
      { entry_type: 'buy', base_currency: 'BTC', base_amount: 0.5.to_d, transacted_at: Time.utc(2024, 3, 1) },
      { entry_type: 'sell', base_currency: 'BTC', base_amount: 1.to_d, transacted_at: Time.utc(2024, 6, 1) }
    ]

    disposals = [
      { asset: 'BTC', acquisition_date: Time.utc(2024, 1, 1), date: Time.utc(2024, 6, 1),
        gain_loss: -5_000.to_d }
    ]

    @report.send(:apply_danish_wash_sale, disposals, enriched)

    assert_equal true, disposals.first[:loss_denied]
  end

  test 'wash sale not triggered when no intervening buy' do
    enriched = [
      { entry_type: 'buy', base_currency: 'BTC', base_amount: 1.to_d, transacted_at: Time.utc(2024, 1, 1) },
      { entry_type: 'sell', base_currency: 'BTC', base_amount: 1.to_d, transacted_at: Time.utc(2024, 6, 1) }
    ]

    disposals = [
      { asset: 'BTC', acquisition_date: Time.utc(2024, 1, 1), date: Time.utc(2024, 6, 1),
        gain_loss: -5_000.to_d }
    ]

    @report.send(:apply_danish_wash_sale, disposals, enriched)

    assert_not disposals.first[:loss_denied]
  end

  test 'wash sale not triggered by different asset buy' do
    enriched = [
      { entry_type: 'buy', base_currency: 'BTC', base_amount: 1.to_d, transacted_at: Time.utc(2024, 1, 1) },
      { entry_type: 'buy', base_currency: 'ETH', base_amount: 10.to_d, transacted_at: Time.utc(2024, 3, 1) },
      { entry_type: 'sell', base_currency: 'BTC', base_amount: 1.to_d, transacted_at: Time.utc(2024, 6, 1) }
    ]

    disposals = [
      { asset: 'BTC', acquisition_date: Time.utc(2024, 1, 1), date: Time.utc(2024, 6, 1),
        gain_loss: -5_000.to_d }
    ]

    @report.send(:apply_danish_wash_sale, disposals, enriched)

    assert_not disposals.first[:loss_denied]
  end

  test 'wash sale not triggered on gains' do
    enriched = [
      { entry_type: 'buy', base_currency: 'BTC', base_amount: 1.to_d, transacted_at: Time.utc(2024, 1, 1) },
      { entry_type: 'buy', base_currency: 'BTC', base_amount: 0.5.to_d, transacted_at: Time.utc(2024, 3, 1) },
      { entry_type: 'sell', base_currency: 'BTC', base_amount: 1.to_d, transacted_at: Time.utc(2024, 6, 1) }
    ]

    disposals = [
      { asset: 'BTC', acquisition_date: Time.utc(2024, 1, 1), date: Time.utc(2024, 6, 1),
        gain_loss: 5_000.to_d }
    ]

    @report.send(:apply_danish_wash_sale, disposals, enriched)

    assert_nil disposals.first[:loss_denied]
  end

  test 'wash sale not triggered when buy is after sale' do
    enriched = [
      { entry_type: 'buy', base_currency: 'BTC', base_amount: 1.to_d, transacted_at: Time.utc(2024, 1, 1) },
      { entry_type: 'sell', base_currency: 'BTC', base_amount: 1.to_d, transacted_at: Time.utc(2024, 6, 1) },
      { entry_type: 'buy', base_currency: 'BTC', base_amount: 0.5.to_d, transacted_at: Time.utc(2024, 7, 1) }
    ]

    disposals = [
      { asset: 'BTC', acquisition_date: Time.utc(2024, 1, 1), date: Time.utc(2024, 6, 1),
        gain_loss: -5_000.to_d }
    ]

    @report.send(:apply_danish_wash_sale, disposals, enriched)

    assert_not disposals.first[:loss_denied]
  end

  test 'wash sale triggered by swap_in of same asset' do
    enriched = [
      { entry_type: 'buy', base_currency: 'BTC', base_amount: 1.to_d, transacted_at: Time.utc(2024, 1, 1) },
      { entry_type: 'swap_in', base_currency: 'BTC', base_amount: 0.2.to_d, transacted_at: Time.utc(2024, 3, 1) },
      { entry_type: 'sell', base_currency: 'BTC', base_amount: 1.to_d, transacted_at: Time.utc(2024, 6, 1) }
    ]

    disposals = [
      { asset: 'BTC', acquisition_date: Time.utc(2024, 1, 1), date: Time.utc(2024, 6, 1),
        gain_loss: -3_000.to_d }
    ]

    @report.send(:apply_danish_wash_sale, disposals, enriched)

    assert_equal true, disposals.first[:loss_denied]
  end

  # --- Per-asset summary tests ---

  test 'danish summary breaks down gains and losses per asset' do
    disposals = [
      { asset: 'BTC', gain_loss: 10_000.to_d, loss_denied: nil },
      { asset: 'BTC', gain_loss: -3_000.to_d, loss_denied: nil },
      { asset: 'ETH', gain_loss: -2_000.to_d, loss_denied: nil },
      { asset: 'ETH', gain_loss: 500.to_d, loss_denied: nil }
    ]

    csv_rows = []
    csv = Object.new
    csv.define_singleton_method(:<<) { |row| csv_rows << row }

    I18n.with_locale(:da) do
      @report.send(:append_danish_summary, csv, disposals)
    end

    # BTC section
    btc_start = csv_rows.index { |r| r == ['BTC:'] }
    assert btc_start, 'BTC section should exist'
    assert_equal 10_000.0, csv_rows[btc_start + 1][1]  # gains
    assert_equal 3_000.0, csv_rows[btc_start + 2][1]   # losses
    assert_equal 780.0, csv_rows[btc_start + 2][3] # 26% deduction

    # ETH section
    eth_start = csv_rows.index { |r| r == ['ETH:'] }
    assert eth_start, 'ETH section should exist'
    assert_equal 500.0, csv_rows[eth_start + 1][1] # gains
    assert_equal 2_000.0, csv_rows[eth_start + 2][1] # losses
    assert_equal 520.0, csv_rows[eth_start + 2][3] # 26% deduction
  end

  test 'denied losses excluded from deduction and shown separately' do
    disposals = [
      { asset: 'BTC', gain_loss: 10_000.to_d, loss_denied: nil },
      { asset: 'BTC', gain_loss: -3_000.to_d, loss_denied: nil },
      { asset: 'BTC', gain_loss: -2_000.to_d, loss_denied: true }
    ]

    csv_rows = []
    csv = Object.new
    csv.define_singleton_method(:<<) { |row| csv_rows << row }

    I18n.with_locale(:da) do
      @report.send(:append_danish_summary, csv, disposals)
    end

    btc_start = csv_rows.index { |r| r == ['BTC:'] }

    # Allowed losses: 3000 (not 5000)
    assert_equal 3_000.0, csv_rows[btc_start + 2][1]
    # 26% of 3000 = 780
    assert_equal 780.0, csv_rows[btc_start + 2][3]
    # Denied losses shown separately
    denied_row = csv_rows.find { |r| r[0]&.include?('uden fradrag') }
    assert denied_row, 'Denied losses row should exist'
    assert_equal 2_000.0, denied_row[1]
  end
end
