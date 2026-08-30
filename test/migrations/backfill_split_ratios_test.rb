require 'test_helper'
require Rails.root.join('db/migrate/20260830210000_backfill_split_ratios.rb')

# The splits already on record predate the ratio and the corporate-action marker, and a sync can
# never revisit them: it resumes near the watermark, and a full re-fetch is discarded as a
# duplicate. Without this they would keep an export that does not say by how much, and a feed that
# never mentions the split at all.
class BackfillSplitRatiosTest < ActiveSupport::TestCase
  setup do
    @user = create(:user)
    @exchange = create(:alpaca_exchange)
    @api_key = create(:api_key, user: @user, exchange: @exchange)
  end

  # A stored merged split: raw_data holds the FIRST leg's activity (the removal, -10 shares) and
  # base_amount holds the net delta (+20), so the old and new counts are both recoverable.
  def stored_split(qty: '-10', net: 20, symbol: 'KLAC', at: 3.days.ago, activity_type: 'SPLIT', **overrides)
    create(:account_transaction, { user: @user, api_key: @api_key, exchange: @exchange,
                                   entry_type: :adjustment, base_currency: symbol, base_amount: net,
                                   quote_currency: nil, quote_amount: nil,
                                   description: "Split (#{symbol})", transacted_at: at,
                                   raw_data: { 'activity_type' => activity_type, 'qty' => qty } }.merge(overrides))
  end

  def holder(symbol: 'KLAC', at: 4.days.ago)
    asset = Asset.find_by(symbol: symbol) || create(:asset, external_id: symbol.downcase, symbol: symbol)
    usd = Asset.find_by(symbol: 'USD') || create(:asset, :usd)
    bot = create(:dca_single_asset, user: @user, exchange: @exchange,
                                    base_asset: asset, quote_asset: usd, with_api_key: false)
    create(:transaction, bot: bot, exchange: @exchange, base: symbol, quote: 'USD', created_at: at)
    bot
  end

  test 'a stored split learns its ratio, its marker and its description' do
    at = stored_split

    migrate!

    at.reload
    assert_equal '3:1', at.raw_data['split_ratio']
    assert_equal 'split', at.raw_data['corporate_action']
    assert_equal 'Split (KLAC) 3:1', at.description
  end

  test 'the ratio is recoverable whichever leg was stored' do
    # The addition leg (+30 new shares) with the same +20 net delta describes the same 3:1.
    at = stored_split(qty: '30')

    migrate!

    assert_equal '3:1', at.reload.raw_data['split_ratio']
  end

  test 'a reverse split reads the other way round' do
    at = stored_split(qty: '-30', net: -20)

    migrate!

    assert_equal '1:3', at.reload.raw_data['split_ratio']
  end

  test 'the bots that were holding it are told' do
    bot = holder
    stored_split

    migrate!

    log = bot.bot_activity_logs.sole
    assert_equal 'asset_split', log.event
    assert_equal '3:1', log.details['ratio']
  end

  test 'a row this already ran over is left exactly as it is' do
    at = stored_split(raw_data: { 'activity_type' => 'SPLIT', 'qty' => '-10',
                                  'corporate_action' => 'split', 'split_ratio' => '3:1' },
                      description: 'Split (KLAC) 3:1')

    migrate!

    assert_equal 'Split (KLAC) 3:1', at.reload.description, 'the ratio must not be appended twice'
  end

  test 'an adjustment that was never a split is untouched' do
    at = stored_split(activity_type: 'JNLC', description: 'Journal')

    migrate!

    at.reload
    assert_nil at.raw_data['corporate_action']
    assert_equal 'Journal', at.description
  end

  test 'a split whose counts do not describe a restatement is still marked' do
    # Removed ten and the delta is minus ten: the position ended at zero, which is not a factor.
    at = stored_split(qty: '-10', net: -10)

    migrate!

    at.reload
    assert_equal 'split', at.raw_data['corporate_action']
    assert_nil at.raw_data['split_ratio'], 'a position that ended at zero has no factor to state'
    assert_equal 'Split (KLAC)', at.description
  end

  def migrate!
    BackfillSplitRatios.new.tap { |m| m.verbose = false }.up
  end
end
