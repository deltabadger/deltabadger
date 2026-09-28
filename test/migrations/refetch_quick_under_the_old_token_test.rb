require 'test_helper'
require Rails.root.join('db/migrate/20260928120000_refetch_quick_under_the_old_token.rb')

# QUICK prices stored before the alias to the old token were fetched as the new one, 1000x too low,
# and storage being insert-only they would stand forever.
class RefetchQuickUnderTheOldTokenTest < ActiveSupport::TestCase
  test 'QUICK prices through the switch are cleared, and every history holding QUICK is rebuilt' do
    HistoricalPrice.create!(asset: 'QUICK', currency: 'USD', date: Date.new(2022, 6, 1), price: 0.07)
    HistoricalPrice.create!(asset: 'QUICK', currency: 'EUR', date: Date.new(2023, 7, 20), price: 0.06)
    kept_quick = HistoricalPrice.create!(asset: 'QUICK', currency: 'USD', date: Date.new(2023, 7, 21), price: 0.06)
    kept_btc = HistoricalPrice.create!(asset: 'BTC', currency: 'USD', date: Date.new(2022, 6, 1), price: 30_000)
    binance = create(:binance_exchange)
    row = lambda do |user, **attrs|
      create(:account_transaction, api_key: create(:api_key, user: user, exchange: binance), **attrs)
    end
    holder, fee_payer, other = create_list(:user, 3)
    row.call(holder, base_currency: 'QUICK')
    row.call(fee_payer, base_currency: 'BTC', fee_currency: 'QUICK', fee_amount: 1)
    row.call(other, base_currency: 'BTC')
    # The backfill rebuilds the stored history (a complete one is never swept again on its own) and
    # recomputes the ledgers when it is done.
    PortfolioSnapshot::BackfillJob.expects(:perform_later).with(holder.id).once
    PortfolioSnapshot::BackfillJob.expects(:perform_later).with(fee_payer.id).once
    PortfolioSnapshot::BackfillJob.expects(:perform_later).with(other.id).never

    ActiveRecord::Migration.suppress_messages { RefetchQuickUnderTheOldToken.new.up }

    assert_equal [kept_quick.id, kept_btc.id].sort, HistoricalPrice.pluck(:id).sort
  end
end
