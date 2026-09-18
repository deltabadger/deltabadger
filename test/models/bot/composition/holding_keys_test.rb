require 'test_helper'

class Bot::Composition::HoldingKeysTest < ActiveSupport::TestCase
  test 'distinct symbols are their own keys' do
    assert_equal({ 1 => 'BTC', 2 => 'ETH', 'SOL' => 'SOL' },
                 Bot::Composition::HoldingKeys.call(1 => 'BTC', 2 => 'ETH', 'SOL' => 'SOL'))
  end

  test 'every owner of a shared symbol is suffixed, so the bare symbol is never a key' do
    keys = Bot::Composition::HoldingKeys.call(12 => 'POR', 57 => 'POR', 99 => 'POR#12')

    assert_equal({ 12 => 'POR#12#12', 57 => 'POR#57', 99 => 'POR#12#99' }, keys)
    assert_not_includes keys.values, 'POR'
  end

  test 'an unresolved string beside a resolved asset of that symbol takes ?' do
    keys = Bot::Composition::HoldingKeys.call(1 => 'BTC', 'BTC' => 'BTC', 7 => 'BTC#?')

    assert_equal 3, keys.values.uniq.size
    assert_equal 'BTC#1', keys[1]
    assert_not_includes keys.values, 'BTC'
  end

  test 'unresolved strings that keep meeting are numbered, and it terminates' do
    keys = Bot::Composition::HoldingKeys.call('A' => 'A', 'A#?' => 'A#?', 3 => 'A')

    assert_equal 3, keys.values.uniq.size
  end
end
