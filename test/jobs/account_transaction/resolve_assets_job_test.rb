require 'test_helper'

# The one-time backfill: rows stored before the ledger recorded assets are resolved by the same rule
# a sync uses, whatever their age, and an asset already on a row is never replaced.
class AccountTransaction::ResolveAssetsJobTest < ActiveSupport::TestCase
  setup do
    @user = create(:user)
    @alpaca = create(:alpaca_exchange)
    @key = create(:api_key, user: @user, exchange: @alpaca)
    @usd = create(:asset, :usd)
    @snps = create(:asset, external_id: 'SNPS.US', symbol: 'SNPS', category: 'Stock')
    create(:ticker, exchange: @alpaca, base_asset: @snps, quote_asset: @usd, base: 'SNPS', quote: 'USD', ticker: 'SNPS')
  end

  def fill(side, **attrs)
    create(:account_transaction, api_key: @key, entry_type: side, base_currency: 'SNPS', quote_currency: 'USD',
                                 raw_data: { 'activity_type' => 'FILL', 'symbol' => 'SNPS' }, **attrs)
  end

  test 'resolves every NULL row of the user, whatever its age' do
    old = travel_to(2.years.ago) { fill(:buy) }
    sold = fill(:sell)
    someone_else = create(:account_transaction, base_currency: 'SNPS', raw_data: { 'activity_type' => 'FILL', 'symbol' => 'SNPS' },
                                                api_key: create(:api_key, exchange: @alpaca))

    AccountTransaction::ResolveAssetsJob.perform_now(@user.id)

    assert_equal [@snps.id, @snps.id], [old.reload.base_asset_id, sold.reload.base_asset_id]
    assert_nil someone_else.reload.base_asset_id, 'another user\'s rows are their own job'
  end

  test 'never replaces a recorded asset, and a second run changes nothing' do
    other = create(:asset, category: 'Stock')
    kept = fill(:buy, base_asset_id: other.id)
    filled = fill(:sell)

    2.times { AccountTransaction::ResolveAssetsJob.perform_now(@user.id) }

    assert_equal other.id, kept.reload.base_asset_id
    assert_equal @snps.id, filled.reload.base_asset_id
  end
end
