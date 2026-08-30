# frozen_string_literal: true

require 'test_helper'

# `decimals` is read per data point while the chart serializes its series, so anything it does
# per call is multiplied by the length of the history. DcaIndex guarded its memoized body with
# `tickers.any?` — an EXISTS query on an unloaded relation, so the guard ran every time even
# though the answer it guarded was computed once: 1457 of the 1463 queries behind one chart
# render were that single check.
class Bots::IndexDecimalsQueriesTest < ActiveSupport::TestCase
  test 'reading decimals repeatedly costs one lookup, not one per read' do
    bot = create(:dca_index, user: create(:user))
    # An unloaded relation is the live shape: `tickers` is built as
    # `exchange.tickers.where(...)` and answers `any?` with an EXISTS query every time.
    bot.stubs(:tickers).returns(Ticker.where(exchange_id: bot.exchange_id))
    bot.decimals # first read may look the tickers up

    assert_no_queries do
      50.times { bot.decimals[:quote] }
    end
  end

  test 'a bot with no tickers still answers, and still only asks once' do
    bot = create(:dca_index, user: create(:user))
    bot.stubs(:tickers).returns(Ticker.none)

    assert_equal({}, bot.decimals)
    assert_no_queries { 10.times { bot.decimals } }
  end
end
