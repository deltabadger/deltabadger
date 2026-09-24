require 'test_helper'

class Bots::FeedShowTest < ActionDispatch::IntegrationTest
  include ActionView::RecordIdentifier

  setup do
    create(:user, admin: true) # satisfies the global setup gate (an admin must exist)
    @bot = create(:dca_single_asset, :started)
    sign_in @bot.user
  end

  test 'renders both transaction and activity rows in the feed' do
    txn = create(:transaction, bot: @bot, external_id: 't1', external_status: :closed, created_at: 1.minute.ago)
    log = @bot.bot_activity_logs.create!(
      event: 'market_closed', created_at: 2.minutes.ago,
      details: { 'next_market_open_at' => Time.utc(2026, 5, 21, 9, 0, 0).iso8601 }
    )

    get bot_path(id: @bot.id, format: :turbo_stream), headers: { 'Turbo-Frame' => 'orders_pagination' }

    assert_response :ok
    # transaction renders twice: a columnar row (Transactions tab) and a timeline sentence (All)
    assert_match dom_id(txn), @response.body
    assert_match dom_id(txn, :timeline), @response.body
    assert_match 'Bought', @response.body
    # activity renders once, in the timeline
    assert_match dom_id(log), @response.body
    assert_match 'Market closed', @response.body
  end

  test 'advances the cursor when more items remain' do
    15.times { |i| @bot.bot_activity_logs.create!(event: 'started', created_at: (20 - i).minutes.ago) }

    get bot_path(id: @bot.id, format: :turbo_stream), headers: { 'Turbo-Frame' => 'orders_pagination' }

    assert_response :ok
    assert_match(/orders_pagination/, @response.body)
    assert_match(/before=/, @response.body)
  end

  test 'failed transaction shows the attempted amounts and the error' do
    create(:transaction, bot: @bot, external_id: 'f1', status: :failed, external_status: :unknown,
                         amount: 0.0001, quote_amount: 10, amount_exec: 0, quote_amount_exec: 0,
                         error_messages: ['This symbol is not permitted for this account.'],
                         created_at: 1.minute.ago)

    get bot_path(id: @bot.id, format: :turbo_stream), headers: { 'Turbo-Frame' => 'orders_pagination' }

    assert_response :ok
    assert_match 'Failed attempt to buy', @response.body
    assert_match '0.0001 BTC', @response.body
    assert_match 'This symbol is not permitted', @response.body
  end

  # The tabs are driven by one attribute per row: data-order-type carries the tabs that row
  # belongs to. Sentence rows are "all", plus "other" when the row has nothing to show in the
  # Amount/Value/Price columns. A columnar row carries its own single tab, or none at all when
  # it has no columnar home — that row then never shows under any tab.

  test 'a filled order is a sentence under All and a columnar row under Transactions' do
    txn = create(:transaction, bot: @bot, external_id: 't1', external_status: :closed, created_at: 1.minute.ago)

    get bot_path(id: @bot.id, format: :turbo_stream), headers: { 'Turbo-Frame' => 'orders_pagination' }

    assert_equal 'all', row_tabs(dom_id(txn, :timeline))
    assert_equal 'successful', row_tabs(dom_id(txn))
  end

  test 'a cancelled order moves to Other as a sentence and keeps no columnar tab' do
    txn = create(:transaction, bot: @bot, external_id: 'c1', external_status: :cancelled, created_at: 1.minute.ago)

    get bot_path(id: @bot.id, format: :turbo_stream), headers: { 'Turbo-Frame' => 'orders_pagination' }

    assert_match 'Order cancelled', @response.body
    assert_equal 'all other', row_tabs(dom_id(txn, :timeline))
    assert_nil row_tabs(dom_id(txn)), 'the columnar row of a cancelled order must belong to no tab'
  end

  test 'skipped and failed orders join the same Other tab' do
    skipped = create(:transaction, :skipped, bot: @bot, created_at: 2.minutes.ago)
    failed = create(:transaction, :failed, bot: @bot, created_at: 1.minute.ago)

    get bot_path(id: @bot.id, format: :turbo_stream), headers: { 'Turbo-Frame' => 'orders_pagination' }

    assert_equal 'all other', row_tabs(dom_id(skipped, :timeline))
    assert_equal 'all other', row_tabs(dom_id(failed, :timeline))
  end

  test 'an activity row shows under All only' do
    log = @bot.bot_activity_logs.create!(event: 'started', created_at: 1.minute.ago)

    get bot_path(id: @bot.id, format: :turbo_stream), headers: { 'Turbo-Frame' => 'orders_pagination' }

    assert_equal 'all', row_tabs(dom_id(log))
  end

  test 'excludes order_skipped activity from the feed' do
    skipped = @bot.bot_activity_logs.create!(event: 'order_skipped', created_at: 1.minute.ago)

    get bot_path(id: @bot.id, format: :turbo_stream), headers: { 'Turbo-Frame' => 'orders_pagination' }

    assert_response :ok
    assert_no_match dom_id(skipped), @response.body
  end

  # The frame's precision is the bot's own, worked out on the server. It used to travel in the
  # frame's URL, and reading it back meant permitting every asset symbol and id in the catalogue.
  test 'the frame renders amounts at the bot precision without being told it' do
    txn = create(:transaction, bot: @bot, external_status: :closed, amount_exec: 0.123456789, created_at: 1.minute.ago)
    base_decimals = @bot.decimals[:base]

    get bot_path(id: @bot.id, format: :turbo_stream), headers: { 'Turbo-Frame' => 'orders_pagination' }

    assert_response :ok
    assert_match dom_id(txn), @response.body
    assert_match "#{0.123456789.round(base_decimals)} <small>#{@bot.base_asset.symbol}</small>", @response.body
  end

  test 'neither the first frame nor the next page carries decimals in its URL' do
    15.times { |i| @bot.bot_activity_logs.create!(event: 'started', created_at: (20 - i).minutes.ago) }

    get bot_path(id: @bot.id)
    first_src = @response.body[/<turbo-frame id="orders_pagination" src="([^"]*)"/, 1]
    assert first_src, 'expected the orders frame on the bot page'
    assert_no_match(/decimals/, CGI.unescapeHTML(first_src))

    get bot_path(id: @bot.id, format: :turbo_stream), headers: { 'Turbo-Frame' => 'orders_pagination' }
    next_src = @response.body[/<turbo-frame id="orders_pagination" src="([^"]*)"/, 1]
    assert next_src, 'expected a next-page frame'
    assert_match(/before=/, next_src)
    assert_no_match(/decimals/, CGI.unescapeHTML(next_src))
  end

  private

  # The tabs the given row belongs to, or nil when it carries none.
  def row_tabs(row_id)
    row = @response.body[%r{<tr id="#{row_id}".*?</tr>}m]
    assert row, "expected a row with id #{row_id} in the feed"
    row[/data-order-type="([^"]*)"/, 1]
  end
end
