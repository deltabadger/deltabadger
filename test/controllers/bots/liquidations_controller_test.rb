require 'test_helper'

# Selling the assets an index has dropped, and clearing the halt when a sale's outcome is unknown.
class Bots::LiquidationsControllerTest < ActionDispatch::IntegrationTest
  def setup
    create(:user, admin: true, setup_completed: true) # onboarding gate
    @user = create(:user)
    @bot = create(:dca_index, user: @user)
    sign_in @user
  end

  test 'the sell is queued rather than run in the request' do
    sellable_quitter

    post bot_liquidation_path(bot_id: @bot.id, symbol: 'CCC')

    assert_predicate queued(Bot::LiquidateExitedJob), :exists?
  end

  test 'who asked is recorded' do
    sellable_quitter

    post bot_liquidation_path(bot_id: @bot.id, symbol: 'CCC')

    log = @bot.bot_activity_logs.find_by(event: 'liquidation_requested')
    assert log, 'a user-initiated sale has to leave a trace'
    assert_equal @user.id, log.details['user_id']
    assert_equal 'CCC', log.details['base'], 'and say which position it was for'
  end

  test 'a closed market says so now instead of failing quietly in a worker' do
    # A one-shot user command must not be silently dropped.
    sellable_quitter
    @bot.exchange.class.any_instance.stubs(:market_open?).returns(false)

    post bot_liquidation_path(bot_id: @bot.id, symbol: 'CCC')

    assert_not_predicate queued(Bot::LiquidateExitedJob), :exists?
    # flash.now, rendered into the turbo_stream response — the modal has to stay open and carry it.
    assert_response :unprocessable_entity
    assert_match(/closed/i, response.body)
  end

  test 'a bot type that cannot have quitters is refused' do
    other = create(:dca_single_asset, user: @user)

    post bot_liquidation_path(bot_id: other.id)

    assert_not_predicate queued(Bot::LiquidateExitedJob), :exists?
  end

  test "another user's bot is not reachable" do
    stranger = create(:dca_single_asset, user: create(:user))

    post bot_liquidation_path(bot_id: stranger.id, symbol: 'CCC')

    assert_not_predicate queued(Bot::LiquidateExitedJob), :exists?
    assert_empty stranger.bot_activity_logs.where(event: 'liquidation_requested')
  end

  # == the confirmation ==
  #
  # A market sale of everything that left the index is the most destructive thing on the page, and a
  # browser confirm() names nothing it is about to sell. It gets the app's own modal, like every
  # other irreversible action here, and the modal lists the positions.

  test 'the confirmation names the one holding it will sell' do
    quitters('AAA' => :in_index, 'CCC' => :exited, 'DDD' => :exited)
    warm_prices('AAA' => 100, 'CCC' => 20, 'DDD' => 30)

    get new_bot_liquidation_path(bot_id: @bot.id, symbol: 'CCC')

    assert_response :success
    assert_select 'turbo-frame#modal .modal', 1
    # The bot page's own table, widget shell included — everything that spaces and aligns a row
    # hangs off .widget--table, so a bare <table> here renders unstyled.
    assert_select '.modal .widget--table #liquidation_confirm_list tr', 1
    assert_select '.modal', /CCC/
    # The other quitter is not part of this sale and must not appear in the list.
    assert_select '.modal', { text: /DDD/, count: 0 }
    assert_select '.modal form[action=?]', bot_liquidation_path(bot_id: @bot.id, symbol: 'CCC'), count: 1
  end

  test 'an in-index holding has no confirmation to open' do
    # The symbol rides in the URL, so it is user input. Selling a live constituent by hand-editing
    # it would be a taxable disposal the index never asked for.
    quitters('AAA' => :in_index, 'CCC' => :exited)
    warm_prices('AAA' => 100, 'CCC' => 20)

    get new_bot_liquidation_path(bot_id: @bot.id, symbol: 'AAA')

    assert_response :not_found
  end

  test 'an unknown symbol has no confirmation to open' do
    quitters('AAA' => :in_index, 'CCC' => :exited)
    warm_prices('AAA' => 100, 'CCC' => 20)

    get new_bot_liquidation_path(bot_id: @bot.id, symbol: 'ZZZ')

    assert_response :not_found
  end

  test 'selling an in-index holding is refused, not queued' do
    quitters('AAA' => :in_index, 'CCC' => :exited)
    warm_prices('AAA' => 100, 'CCC' => 20)

    post bot_liquidation_path(bot_id: @bot.id, symbol: 'AAA')

    assert_response :not_found
    assert_not_predicate queued(Bot::LiquidateExitedJob), :exists?
  end

  test 'the sale is queued for the named holding only' do
    quitters('AAA' => :in_index, 'CCC' => :exited)
    warm_prices('AAA' => 100, 'CCC' => 20)

    post bot_liquidation_path(bot_id: @bot.id, symbol: 'CCC')

    job = queued(Bot::LiquidateExitedJob).last
    assert job, 'the sale has to reach the queue'
    assert_match(/CCC/, job.arguments.to_s)
  end

  test 'opening the confirmation sells nothing on its own' do
    quitters('AAA' => :in_index, 'CCC' => :exited)
    warm_prices('AAA' => 100, 'CCC' => 20)

    get new_bot_liquidation_path(bot_id: @bot.id, symbol: 'CCC')

    assert_not_predicate queued(Bot::LiquidateExitedJob), :exists?
    assert_empty @bot.bot_activity_logs.where(event: 'liquidation_requested')
  end

  test 'a bot type that cannot have quitters has no confirmation to open' do
    other = create(:dca_single_asset, user: @user)

    get new_bot_liquidation_path(bot_id: other.id, symbol: 'CCC')

    assert_response :redirect
  end

  test "another user's confirmation is not reachable" do
    stranger = create(:dca_index, user: create(:user), exchange: @bot.exchange, quote_asset: @bot.quote_asset)

    get new_bot_liquidation_path(bot_id: stranger.id, symbol: 'CCC')

    assert_response :not_found
  end

  # == resolution ==

  test 'clearing a halt is queued behind the exchange semaphore, not done inline' do
    @bot.start_liquidation_placement!('CCC')
    @bot.flag_liquidation_ambiguous!

    post bot_liquidation_resolutions_path(bot_id: @bot.id, intent_id: @bot.liquidation_pending[:id])

    assert_predicate queued(Bot::ResolveLiquidationJob), :exists?
  end

  test 'a halt is not clearable while one of its orders is still working' do
    # The whole point of the halt is that an order MAY be live; clearing on top of one we can still
    # see would let the next attempt sell the same coins again.
    asset = create(:asset, symbol: 'CCC', name: 'Coin CCC', external_id: 'coin-ccc')
    create(:ticker, exchange: @bot.exchange, base_asset: asset, quote_asset: @bot.quote_asset)
    @bot.start_liquidation_placement!('CCC')
    @bot.flag_liquidation_ambiguous!
    create(:transaction, bot: @bot, exchange: @bot.exchange, status: :submitted, external_status: :open,
                         external_id: 'live-1', side: :sell, base: 'CCC', quote: @bot.quote_asset.symbol,
                         transaction_type: 'LIQUIDATION', price: 100, amount: 1)
    Bot::FetchAndUpdateOrderJob.any_instance.stubs(:perform)

    post bot_liquidation_resolutions_path(bot_id: @bot.id, intent_id: @bot.liquidation_pending[:id])

    assert_not_predicate queued(Bot::ResolveLiquidationJob), :exists?
    assert_match(/live-1/, flash[:alert])
  end

  private

  # Solid Queue is the adapter here, so ActiveJob's test-adapter assertions are unavailable.
  def queued(job_class)
    SolidQueue::Job.where(class_name: job_class.name)
  end

  # The smallest setup that makes CCC a sellable quitter: an index with one member, so the
  # composition is known, and a holding outside it.
  def sellable_quitter
    quitters('AAA' => :in_index, 'CCC' => :exited)
    warm_prices('AAA' => 100, 'CCC' => 20)
  end

  # in_index vs exited, in the shape exited_holdings reads.
  def quitters(roles)
    roles.each do |symbol, role|
      asset = create(:asset, symbol: symbol, name: "Coin #{symbol}", external_id: "coin-#{symbol.downcase}")
      ticker = create(:ticker, exchange: @bot.exchange, base_asset: asset, quote_asset: @bot.quote_asset)
      BotIndexAsset.create!(bot: @bot, asset: asset, ticker: ticker, in_index: role == :in_index,
                            target_allocation: role == :in_index ? 1.0 : nil,
                            entered_at: Time.current, exited_at: role == :in_index ? nil : Time.current)
    end
  end

  def warm_prices(values)
    Rails.stubs(:cache).returns(ActiveSupport::Cache::MemoryStore.new)
    data = @bot.metrics.deep_dup
    data[:asset_values] = values.transform_values do |v|
      { amount: v.to_d / 100, quote_invested: v.to_d, current_value: v.to_d,
        current_price: 100, avg_price: 100, pnl_percentage: 0 }
    end
    # The ledger underneath, which is what exited_symbols reads — see the quitters-table test.
    data[:asset_breakdown] = values.transform_values { |v| { amount: v.to_d / 100, quote_invested: v.to_d } }
    Rails.cache.write("bot_#{@bot.id}_metrics_v3", data)
    Rails.cache.write(@bot.send(:metrics_with_current_prices_cache_key), data)
  end
end
