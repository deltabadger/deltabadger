require 'test_helper'

class Bot::LimitOrderableTest < ActiveSupport::TestCase
  test 'defaults limit_ordered to false on non-Hyperliquid exchanges' do
    bot = build(:dca_single_asset)

    assert_not bot.limit_ordered?
  end

  test 'defaults limit_ordered to true on Hyperliquid' do
    bot = build(:dca_single_asset, exchange: build(:hyperliquid_exchange))

    assert bot.limit_ordered?
  end

  test 'rejects limit_ordered=false on Hyperliquid' do
    bot = build(:dca_single_asset, exchange: build(:hyperliquid_exchange))
    bot.limit_ordered = false

    assert_not bot.valid?
    assert bot.errors[:limit_ordered].present?
  end

  test 'allows limit_ordered=false on non-Hyperliquid exchanges' do
    bot = build(:dca_single_asset)
    bot.limit_ordered = false

    assert bot.valid?, bot.errors.full_messages.to_sentence
  end

  test 'parse_params cannot turn limit_ordered off on Hyperliquid' do
    bot = build(:dca_single_asset, exchange: build(:hyperliquid_exchange))

    parsed = bot.parse_params(ActionController::Parameters.new(limit_ordered: '0').permit!)

    assert_equal true, parsed[:limit_ordered]
  end

  test 'parse_params honors limit_ordered=0 on non-Hyperliquid exchanges' do
    bot = build(:dca_single_asset)

    parsed = bot.parse_params(ActionController::Parameters.new(limit_ordered: '0').permit!)

    assert_equal false, parsed[:limit_ordered]
  end

  test 'DcaDualAsset on Hyperliquid defaults limit_ordered true and rejects false' do
    bot = build(:dca_dual_asset, exchange: build(:hyperliquid_exchange))

    assert bot.limit_ordered?

    bot.limit_ordered = false
    assert_not bot.valid?
    assert bot.errors[:limit_ordered].present?
  end

  # == limit_order_pcnt_distance_decimal coercion ==

  test 'limit_order_pcnt_distance_decimal returns BigDecimal for String input' do
    bot = build(:dca_single_asset)
    bot.settings = bot.settings.merge('limit_order_pcnt_distance' => '0.005')

    result = bot.send(:limit_order_pcnt_distance_decimal)
    assert_kind_of BigDecimal, result
    assert_equal BigDecimal('0.005'), result
  end

  test 'limit_order_pcnt_distance_decimal returns BigDecimal for Float input' do
    bot = build(:dca_single_asset)
    bot.limit_order_pcnt_distance = 0.005

    result = bot.send(:limit_order_pcnt_distance_decimal)
    assert_kind_of BigDecimal, result
    assert_equal BigDecimal('0.005'), result
  end

  test 'limit_order_pcnt_distance_decimal falls back to 0.001 default when nil' do
    bot = build(:dca_single_asset)
    bot.settings = bot.settings.merge('limit_order_pcnt_distance' => nil)

    result = bot.send(:limit_order_pcnt_distance_decimal)
    assert_kind_of BigDecimal, result
    assert_equal BigDecimal('0.001'), result
  end

  test 'limit_order_pcnt_distance_decimal falls back to 0.001 default when empty string' do
    bot = build(:dca_single_asset)
    bot.settings = bot.settings.merge('limit_order_pcnt_distance' => '')

    result = bot.send(:limit_order_pcnt_distance_decimal)
    assert_equal BigDecimal('0.001'), result
  end
end
