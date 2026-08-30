require 'test_helper'

class Bot::Composition::WeightableTest < ActiveSupport::TestCase
  test 'pure market cap weighting is proportional' do
    weights = Bot::Composition::Weightable.blend(market_caps: { 1 => 750.0, 2 => 250.0 }, flattening: 0)

    assert_in_delta 0.75, weights[1], 0.0001
    assert_in_delta 0.25, weights[2], 0.0001
  end

  test 'full flattening is equal weighting' do
    weights = Bot::Composition::Weightable.blend(market_caps: { 1 => 900.0, 2 => 100.0 }, flattening: 1)

    assert_in_delta 0.5, weights[1], 0.0001
    assert_in_delta 0.5, weights[2], 0.0001
  end

  test 'weights always sum to one' do
    weights = Bot::Composition::Weightable.blend(
      market_caps: { 1 => 500.0, 2 => 300.0, 3 => 200.0 }, flattening: 0.4
    )

    assert_in_delta 1.0, weights.values.sum, 0.0001
  end

  test 'a zero total falls back to equal weights rather than dividing by zero' do
    weights = Bot::Composition::Weightable.blend(market_caps: { 1 => 0.0, 2 => 0.0 }, flattening: 0)

    assert_in_delta 0.5, weights[1], 0.0001
    assert_in_delta 0.5, weights[2], 0.0001
  end

  test 'flattening outside 0..1 is clamped, not extrapolated' do
    weights = Bot::Composition::Weightable.blend(market_caps: { 1 => 900.0, 2 => 100.0 }, flattening: 5)

    assert_in_delta 0.5, weights[1], 0.0001
  end

  test 'an empty set is empty, not a division by zero' do
    assert_empty Bot::Composition::Weightable.blend(market_caps: {}, flattening: 0)
  end
end
