require 'test_helper'

class Tax::GenerateReportJobTest < ActiveSupport::TestCase
  # A report that omits a whole exchange must say so on its face. Anything less lets a user file a
  # document that looks complete while an exchange contributed nothing.
  test 'a failed exchange sync banners the report with the exchange name' do
    user = create(:user)
    create(:api_key, user: user, exchange: create(:binance_exchange),
                     last_synced_at: 1.day.ago, last_sync_error: 'StandardError: API error')

    rows = generate(user, 'DE', 2024)

    assert_includes rows[1].first, 'Binance'
  end

  test 'a connected but never-synced exchange banners the report too' do
    user = create(:user)
    create(:api_key, user: user, exchange: create(:kraken_exchange), last_synced_at: nil)

    rows = generate(user, 'DE', 2023)

    assert_includes rows[1].first, 'Kraken'
  end

  test 'healthy, withdrawal-only and stock-venue keys leave the report unbannered' do
    user = create(:user)
    create(:api_key, user: user, exchange: create(:binance_exchange), last_synced_at: 1.day.ago)
    create(:api_key, user: user, exchange: create(:kraken_exchange), key_type: :withdrawal, last_synced_at: nil)
    # Stock venues are excluded from the crypto report, so a broken Alpaca key cannot hide data in it.
    create(:api_key, user: user, exchange: create(:alpaca_exchange),
                     last_synced_at: nil, last_sync_error: 'StandardError: API error')

    rows = generate(user, 'DE', 2022)

    assert_not_includes rows.flatten.compact.join(' '), 'Alpaca'
    assert_equal 2, rows.size, 'headers plus the no-transactions line, with no warning banner'
  end

  private

  # Parallel workers each get their own database but share tmp/, so two tests generating the same
  # (user_id, country, year) would clobber each other's file. One year per test keeps them apart.
  def generate(user, country, year)
    Tax::GenerateReportJob.perform_now(user.id, country, year)
    CSV.parse(File.read(Rails.root.join('tmp', 'tax_reports', "#{user.id}_#{country}_#{year}.csv")))
  end
end
