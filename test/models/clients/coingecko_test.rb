require 'test_helper'

# `@detect_plan ||= ...` is a class-level memo that ignores its argument, so the FIRST key seen in a
# process pinned the plan — and with it the base URL and the header name — for every later key.
class Clients::CoingeckoTest < ActiveSupport::TestCase
  teardown { Clients::Coingecko.instance_variable_set(:@detect_plan, nil) }

  test 'detect_plan is memoised per key, not globally' do
    Clients::Coingecko.stubs(:pro_key?).with('pro_key').returns(true)
    Clients::Coingecko.stubs(:pro_key?).with('demo_key').returns(false)

    assert_equal :pro, Clients::Coingecko.detect_plan('pro_key')
    assert_equal :demo, Clients::Coingecko.detect_plan('demo_key')
    assert_equal :pro, Clients::Coingecko.detect_plan('pro_key')
  end

  test 'the plan for one key is only probed once' do
    Clients::Coingecko.expects(:pro_key?).with('demo_key').once.returns(false)

    3.times { assert_equal :demo, Clients::Coingecko.detect_plan('demo_key') }
  end
end
