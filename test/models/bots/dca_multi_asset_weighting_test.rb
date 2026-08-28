require 'test_helper'

# A basket can hold its weights two ways: sliders the user drags, or market caps it derives. The
# derived mode is what the retired pair bot's market-cap switch becomes — with one deliberate
# difference, pinned below: weights come from the stored market_cap column, never a live price,
# because derivation runs inside the synchronous after_save reconciliation.
class Bots::DcaMultiAssetWeightingTest < ActiveSupport::TestCase
  setup do
    @btc = create(:asset, :bitcoin, market_cap: 750.0)
    @eth = create(:asset, :ethereum, market_cap: 250.0)
    @bot = create(:dca_multi_asset, base_assets: [@btc, @eth])
  end

  # Bot::Accountable raises on a settings save without set_missed_quote_amount (accountable.rb:82).
  def configure!(**attributes)
    @bot.assign_attributes(attributes)
    @bot.set_missed_quote_amount
    @bot.save!
    @bot
  end

  def weight_of(asset)
    @bot.bot_index_assets.find_by(asset:).target_allocation.to_f
  end

  test 'a basket is manually weighted unless told otherwise' do
    assert_equal 'manual', @bot.weighting
    assert_not @bot.market_cap_weighted?
    assert_not @bot.settings.key?('weighting')
  end

  test 'a manual basket keeps the weights the user set' do
    configure!(allocations: { @btc.id.to_s => 0.3, @eth.id.to_s => 0.7 })
    @bot.refresh_composition

    assert_in_delta 0.3, weight_of(@btc), 0.0001
  end

  test 'a market-cap basket derives its weights and ignores the stored sliders' do
    configure!(allocations: { @btc.id.to_s => 0.3, @eth.id.to_s => 0.7 }, weighting: 'market_cap')
    @bot.refresh_composition

    assert_in_delta 0.75, weight_of(@btc), 0.0001
    assert_in_delta 0.25, weight_of(@eth), 0.0001
  end

  test 'a missing market cap falls back to the stored weights, never to equal weights' do
    @eth.update!(market_cap: nil)
    configure!(allocations: { @btc.id.to_s => 0.3, @eth.id.to_s => 0.7 }, weighting: 'market_cap')
    @bot.refresh_composition

    assert_in_delta 0.3, weight_of(@btc), 0.0001
    assert_in_delta 0.7, weight_of(@eth), 0.0001
  end

  test 'deriving weights makes no exchange call' do
    configure!(weighting: 'market_cap')
    @bot.exchange.expects(:get_last_price).never
    @bot.exchange.expects(:get_tickers_prices).never

    assert @bot.refresh_composition.success?
  end

  test 'a market-cap basket is always balanced, so it never blocks start on the slider check' do
    configure!(allocations: { @btc.id.to_s => 0.3, @eth.id.to_s => 0.1 }, weighting: 'market_cap')

    assert @bot.allocations_balanced?
  end

  test 'the displayed weights are the derived ones, not the stored sliders' do
    configure!(allocations: { @btc.id.to_s => 0.3, @eth.id.to_s => 0.7 }, weighting: 'market_cap')
    @bot.refresh_composition

    assert_in_delta 0.75, @bot.displayed_allocation_for(@btc.id), 0.0001
    assert_in_delta 1.0, @bot.displayed_allocations_total, 0.0001
  end

  test 'a manual basket displays its sliders, off-100 total included' do
    configure!(allocations: { @btc.id.to_s => 0.3, @eth.id.to_s => 0.1 })

    assert_in_delta 0.3, @bot.displayed_allocation_for(@btc.id), 0.0001
    assert_in_delta 0.4, @bot.displayed_allocations_total, 0.0001
  end

  test 'switching to market cap reconciles the membership on save, not on the next tick' do
    configure!(allocations: { @btc.id.to_s => 0.5, @eth.id.to_s => 0.5 })
    configure!(weighting: 'market_cap')

    assert_in_delta 0.75, weight_of(@btc), 0.0001
  end

  # == when the rule may be offered at all ==

  test 'a basket of assets that all carry a market cap can be weighted by it' do
    assert @bot.market_cap_weightable?
  end

  test 'a basket holding a stock cannot — stocks carry no market cap' do
    # The case that prompted this: QQQM and IBIT are both stocks, market_cap nil on each, so the
    # rule would have been a switch that changed nothing.
    @eth.update!(market_cap: nil, category: 'Stock')

    assert_not @bot.market_cap_weightable?
  end

  test 'a market cap of zero counts as missing, not as a weight' do
    @eth.update!(market_cap: 0)

    assert_not @bot.market_cap_weightable?
  end

  test 'a basket already weighted by market cap still says so when its data goes away' do
    # Otherwise the rule vanishes while still switched on, with no way to turn it off.
    configure!(weighting: 'market_cap')
    @eth.update!(market_cap: nil)

    assert_not @bot.market_cap_weightable?
    assert @bot.market_cap_weighted?
  end

  test 'an unknown weighting is refused' do
    @bot.weighting = 'astrology'
    @bot.set_missed_quote_amount

    assert_not @bot.valid?
    assert_predicate @bot.errors[:weighting], :present?
  end
end
