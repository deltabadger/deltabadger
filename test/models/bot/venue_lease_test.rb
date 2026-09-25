# frozen_string_literal: true

require 'test_helper'

class Bot::VenueLeaseTest < ActiveSupport::TestCase
  setup do
    @exchange = create(:binance_exchange)
    @key = Bot::VenueLease::ExchangeLease.for(@exchange).concurrency_key
  end

  test 'holds the venue semaphore while the block runs and hands it back' do
    held = nil
    assert Bot::VenueLease.hold([@exchange], holder: 'test') { held = SolidQueue::Semaphore.find_by(key: @key).value }

    assert_equal 0, held
    assert_equal 1, SolidQueue::Semaphore.find_by(key: @key).value
  end

  test 'a taken lease refuses without running the block' do
    SolidQueue::Semaphore.create!(key: @key, value: 0, expires_at: 5.minutes.from_now)
    ran = false

    assert_not Bot::VenueLease.hold([@exchange], holder: 'test') { ran = true }
    assert_not ran
  end

  test 'a holder that outlives its lease does not signal a semaphore that may be someone elses' do
    Bot::VenueLease.hold([@exchange], holder: 'test') { travel(Bot::VenueLease::LEASE + 1.second) }

    assert_equal 0, SolidQueue::Semaphore.find_by(key: @key).value, 'left for the queue to expire, never signalled'
  ensure
    travel_back
  end
end
