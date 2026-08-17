require 'test_helper'

class FundClassificationTest < ActiveSupport::TestCase
  test 'enum integer mappings are stable stored values' do
    assert_equal({ 'share' => 0, 'fund' => 1, 'other_security' => 2 }, FundClassification.kinds)
    assert_equal(
      {
        'equity_fund' => 0,
        'mixed_fund' => 1,
        'real_estate_fund' => 2,
        'foreign_real_estate_fund' => 3,
        'other_fund' => 4
      },
      FundClassification.fund_categories
    )
  end

  test 'resolve proposes other fund for an ETF and copies its ISIN' do
    user = create(:user)
    asset = create(:asset, instrument_type: 'etf', isin: 'US78462F1030')

    classification = FundClassification.resolve(user: user, symbol: asset.symbol, asset: asset)

    assert_equal 'fund', classification.kind
    assert_equal 'other_fund', classification.fund_category
    assert_equal asset.isin, classification.isin
    refute_predicate classification, :persisted?
  end

  test 'resolve proposes a share for a stock' do
    user = create(:user)
    asset = create(:asset, instrument_type: 'stock')

    classification = FundClassification.resolve(user: user, symbol: asset.symbol, asset: asset)

    assert_equal 'share', classification.kind
    assert_nil classification.fund_category
    refute_predicate classification, :persisted?
  end

  test 'resolve leaves an asset with nil instrument type unclassified' do
    user = create(:user)
    asset = create(:asset, instrument_type: nil)

    classification = FundClassification.resolve(user: user, symbol: asset.symbol, asset: asset)

    assert_nil classification.kind
    refute_predicate classification, :persisted?
  end

  test 'resolve leaves an unrecognised instrument type unclassified' do
    user = create(:user)
    asset = create(:asset, instrument_type: 'bond')

    classification = FundClassification.resolve(user: user, symbol: asset.symbol, asset: asset)

    assert_nil classification.kind
    refute_predicate classification, :persisted?
  end

  test 'resolve without an asset leaves the proposal unclassified with no ISIN' do
    user = create(:user)

    classification = FundClassification.resolve(user: user, symbol: 'UNKNOWN')

    assert_nil classification.kind
    assert_nil classification.isin
    refute_predicate classification, :persisted?
  end

  test 'a proposal is not attached to the user association, so saving the user cannot persist it' do
    user = create(:user)
    asset = create(:asset, instrument_type: nil)

    FundClassification.resolve(user: user, symbol: asset.symbol, asset: asset)
    user.save!

    assert_empty user.fund_classifications.reload
  end

  test 'resolve returns a persisted user override instead of the asset proposal' do
    user = create(:user)
    asset = create(:asset, instrument_type: 'etf')
    override = user.fund_classifications.create!(
      symbol: asset.symbol,
      isin: asset.isin,
      kind: :fund,
      fund_category: :equity_fund
    )

    classification = FundClassification.resolve(user: user, symbol: asset.symbol, asset: asset)

    assert_equal override.id, classification.id
    assert_equal 'equity_fund', classification.fund_category
    assert_predicate classification, :persisted?
  end

  test 'resolve does not return another users classification for the same symbol' do
    user = create(:user)
    other_user = create(:user)
    asset = create(:asset, instrument_type: 'stock')
    other_classification = other_user.fund_classifications.create!(symbol: asset.symbol, kind: :share)

    classification = FundClassification.resolve(user: user, symbol: asset.symbol, asset: asset)

    refute_equal other_classification.id, classification.id
    assert_equal user, classification.user
    assert_equal 'share', classification.kind
    refute_predicate classification, :persisted?
  end

  test 'symbol uniqueness is scoped to each user and enforced by the database' do
    user = create(:user)
    other_user = create(:user)
    FundClassification.create!(user: user, symbol: 'SPY', kind: :share)

    other_classification = FundClassification.create!(user: other_user, symbol: 'SPY', kind: :share)
    assert_predicate other_classification, :persisted?

    duplicate = FundClassification.new(user: user, symbol: 'SPY', kind: :share)
    refute_predicate duplicate, :valid?
    assert duplicate.errors.added?(:symbol, :taken, value: 'SPY')
    assert_raises(ActiveRecord::RecordNotUnique) { duplicate.save!(validate: false) }
  end

  test 'funds require a fund category and shares forbid one' do
    user = create(:user)
    fund = FundClassification.new(user: user, symbol: 'FUND', kind: :fund)
    share = FundClassification.new(user: user, symbol: 'SHARE', kind: :share, fund_category: :equity_fund)

    refute_predicate fund, :valid?
    assert fund.errors.added?(:fund_category, :blank)
    refute_predicate share, :valid?
    assert share.errors.added?(:fund_category, :present)
  end
end
