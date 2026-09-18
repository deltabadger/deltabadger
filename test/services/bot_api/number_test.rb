# frozen_string_literal: true

require 'test_helper'

class BotApi::NumberTest < ActiveSupport::TestCase
  test 'plain decimals, exactly' do
    assert_equal BigDecimal('0.1'), BotApi::Number.parse('0.1')
    assert_equal BigDecimal('100'), BotApi::Number.parse(' 100 ')
    assert_equal 7, BotApi::Number.integer('7')
    # MCP casts `type: 'number'` properties to Float before a service ever sees them.
    assert_equal 2025, BotApi::Number.integer(2025.0)
    assert_equal BigDecimal('50'), BotApi::Number.parse(50.0)
  end

  test 'everything else is nil, including what to_f would have swallowed' do
    ['abc', '', nil, '-5', '1e9', '1,5', '5.', '.5', '1' * 16, "1.#{'0' * 19}", '5.5', 12.5].each do |bad|
      assert_nil BotApi::Number.integer(bad), bad.inspect
    end
    ['abc', '', nil, '-5', '1e9', '1,5', '5.', '.5', '1' * 16, "1.#{'0' * 19}", 'Infinity', 'NaN'].each do |bad|
      assert_nil BotApi::Number.parse(bad), bad.inspect
    end
  end

  test 'within' do
    assert_equal BigDecimal('50'), BotApi::Number.within('50', 0.0..100.0)
    assert_nil BotApi::Number.within('100.01', 0.0..100.0)
    assert_nil BotApi::Number.within('abc', 0.0..100.0)
  end

  # A number that arrived AS a number — JSON, or an MCP `number` property — is a Float, and a small
  # Float prints in scientific notation. That is not text anybody typed; it is 0.00005.
  test 'a small Float is the decimal it is; typed text stays strict' do
    assert_equal BigDecimal('0.00005'), BotApi::Number.parse(0.00005)
    assert_equal BigDecimal('0.3'), BotApi::Number.parse(0.1 + 0.2)
    assert_nil BotApi::Number.parse('5.0e-05'), 'typed exponents are still refused'
    [-0.5, 1e20, Float::INFINITY, Float::NAN].each { |bad| assert_nil BotApi::Number.parse(bad), bad.inspect }
  end

  # Writing a Float out as a decimal rounds to 16 significant digits. An id is asked of the Float
  # itself, or 1.0000000000000002 would name bot 1.
  test 'a Float that is not whole is never an integer, however close' do
    assert_nil BotApi::Number.integer(1.0000000000000002)
    assert_nil BotApi::Number.integer(Float::INFINITY)
    assert_nil BotApi::Number.integer(Float::NAN)
    assert_equal 12, BotApi::Number.integer(12.0)
  end
end
