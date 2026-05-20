require 'test_helper'

class BotActivityFeedTest < ActiveSupport::TestCase
  setup do
    @bot = create(:dca_single_asset)
  end

  def activity(at:, event: 'started')
    @bot.bot_activity_logs.create!(event: event, created_at: at)
  end

  def txn(at:, ext:)
    create(:transaction, bot: @bot, external_id: ext, created_at: at)
  end

  test 'merges transactions and activity logs into one newest-first list' do
    t0 = Time.utc(2026, 5, 1, 12, 0, 0)
    a = activity(at: t0)
    b = txn(at: t0 + 1.minute, ext: 't1')
    c = activity(at: t0 + 2.minutes, event: 'market_closed')

    assert_equal [c, b, a], BotActivityFeed.new(bot: @bot, limit: 10).items
  end

  test 'respects the limit' do
    t0 = Time.utc(2026, 5, 1, 12, 0, 0)
    5.times { |i| activity(at: t0 + i.minutes) }

    assert_equal 3, BotActivityFeed.new(bot: @bot, limit: 3).items.size
  end

  test 'next_cursor is nil when there are no more items' do
    activity(at: Time.utc(2026, 5, 1, 12, 0, 0))

    assert_nil BotActivityFeed.new(bot: @bot, limit: 10).next_cursor
  end

  test 'next_cursor is present when more items remain' do
    t0 = Time.utc(2026, 5, 1, 12, 0, 0)
    3.times { |i| activity(at: t0 + i.minutes) }

    assert_not_nil BotActivityFeed.new(bot: @bot, limit: 2).next_cursor
  end

  test 'paginating with the cursor walks every item exactly once, newest-first' do
    t0 = Time.utc(2026, 5, 1, 12, 0, 0)
    records = []
    7.times do |i|
      records << (i.even? ? activity(at: t0 + i.minutes, event: "e#{i}") : txn(at: t0 + i.minutes, ext: "t#{i}"))
    end
    expected = records.sort_by(&:created_at).reverse.map { |r| [r.class.name, r.id] }

    collected = []
    cursor = nil
    loop do
      feed = BotActivityFeed.new(bot: @bot, before: cursor, limit: 2)
      collected.concat(feed.items)
      cursor = feed.next_cursor
      break if cursor.nil?
    end

    assert_equal(expected, collected.map { |r| [r.class.name, r.id] })
    assert_equal collected.size, collected.map { |r| [r.class.name, r.id] }.uniq.size, 'no duplicates across pages'
  end

  test 'walks every record exactly once when many rows share one created_at (high cardinality)' do
    t = Time.utc(2026, 5, 1, 12, 0, 0)
    records = Array.new(25) { |i| activity(at: t, event: "e#{i}") }

    collected = []
    cursor = nil
    loop do
      feed = BotActivityFeed.new(bot: @bot, before: cursor, limit: 3)
      collected.concat(feed.items)
      cursor = feed.next_cursor
      break if cursor.nil?
    end

    ids = collected.map { |r| [r.class.name, r.id] }
    assert_equal records.size, collected.size, 'no records skipped'
    assert_equal ids.uniq.size, ids.size, 'no duplicates across pages'
    assert_equal records.map { |r| [r.class.name, r.id] }.sort, ids.sort
  end

  test 'records sharing a created_at use a stable documented tiebreaker across pages' do
    # Documented feed order: created_at desc, kind asc (activity before transaction), id desc.
    t = Time.utc(2026, 5, 1, 12, 0, 0)
    a = activity(at: t)
    b = txn(at: t, ext: 't1')

    collected = []
    cursor = nil
    loop do
      feed = BotActivityFeed.new(bot: @bot, before: cursor, limit: 1)
      collected.concat(feed.items)
      cursor = feed.next_cursor
      break if cursor.nil?
    end

    assert_equal [[a.class.name, a.id], [b.class.name, b.id]],
                 collected.map { |r| [r.class.name, r.id] },
                 'activity should sort before transaction at an identical created_at'
  end

  test 'excludes order_skipped/order_ignored but keeps execution_failed' do
    t0 = Time.utc(2026, 5, 1, 12, 0, 0)
    started = activity(at: t0, event: 'started')
    activity(at: t0 + 1.minute, event: 'order_skipped')
    activity(at: t0 + 2.minutes, event: 'order_ignored')
    failed = activity(at: t0 + 3.minutes, event: 'execution_failed')

    items = BotActivityFeed.new(bot: @bot, limit: 10).items
    assert_includes items, failed, 'execution_failed (e.g. auth/market errors) must stay visible'
    assert_includes items, started
    assert_equal 2, items.size
  end

  test 'scopes to the given bot' do
    other = create(:dca_single_asset, user: @bot.user, exchange: @bot.exchange,
                                      base_asset: @bot.base_asset, quote_asset: @bot.quote_asset)
    other.bot_activity_logs.create!(event: 'started')
    mine = activity(at: Time.utc(2026, 5, 1, 12, 0, 0))

    assert_equal [mine], BotActivityFeed.new(bot: @bot, limit: 10).items
  end
end
