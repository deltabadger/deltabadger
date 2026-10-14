# frozen_string_literal: true

require 'test_helper'

# update_bot_settings (MCP and REST, through BotApi::Bots::UpdateSettings) changes a bot exactly the way the
# settings page does. Each case saves the page's post on one bot and the automation call on an identical twin,
# and requires BOTH to succeed with the case's EXPLICIT expected change — every other setting unchanged — plus
# identical composition rows, enqueued jobs and broadcasts. Two paths that lost the same behaviour would miss
# the expected change, so they cannot pass together.
class Bots::SettingsParityTest < ActionDispatch::IntegrationTest
  NOW = Time.utc(2026, 6, 3, 12, 0, 0)

  setup do
    travel_to NOW
    @user = create(:user, admin: true, setup_completed: true, time_zone: 'UTC')
    sign_in @user
    @exchange = create(:binance_exchange)
    @usd = create(:asset, :usd)
    @btc = create(:asset, :bitcoin, market_cap: 750.0)
    @eth = create(:asset, :ethereum, market_cap: 250.0)
    @sol = create(:asset, external_id: 'solana', symbol: 'SOL', name: 'Solana', market_cap: 50.0)
    [@btc, @eth, @sol].each { |asset| create(:ticker, exchange: @exchange, base_asset: asset, quote_asset: @usd) }
    @eur = create(:asset, :eur)
  end

  def basket(settings = {}, assets: [@btc, @eth], **attrs)
    bot = create(:dca_multi_asset, :stopped, user: @user, exchange: @exchange, base_assets: assets, quote_asset: @usd, **attrs)
    return bot if settings.empty?

    bot.settings = bot.settings.merge(settings.stringify_keys)
    bot.set_missed_quote_amount
    bot.save!
    bot
  end

  def index_bot(settings = {})
    bot = create(:dca_index, user: @user, status: :stopped, quote_asset: @eur, exchange: @index_exchange ||= create(:kraken_exchange))
    return bot if settings.empty?

    bot.settings = bot.settings.merge(settings.stringify_keys)
    bot.set_missed_quote_amount
    bot.save!
    bot
  end

  def ticker(asset) = Ticker.find_by!(exchange: @exchange, base_asset: asset, quote_asset: @usd)

  # Everything a save can leave behind, with the bot's own id taken out so twins compare.
  def effects(bot)
    bot.reload
    { settings: bot.settings, transient: bot.transient_data.except('missed_quote_amount'), label: bot.label,
      exchange_id: bot.exchange_id,
      composition: bot.bot_index_assets.order(:asset_id).map { |m| m.attributes.slice('asset_id', 'ticker_id', 'in_index', 'target_allocation') } }
  end

  def recorded(bot)
    last_job = SolidQueue::Job.maximum(:id).to_i
    ActionCable.server.pubsub.clear
    yield
    jobs = SolidQueue::Job.where('id > ?', last_job).map do |job|
      [job.class_name, job.arguments['arguments'].to_json.gsub(bot.to_gid.to_s, 'BOT')]
    end.sort
    streams = ActionCable.server.pubsub.instance_variable_get(:@channels_data).to_h
                         .transform_keys { |name| name.to_s.gsub(bot.to_gid_param, 'BOT') }.transform_values(&:size)
    { jobs:, streams: }
  end

  # Saves `form` through the page on one bot and `args` through the API on its twin; both must succeed with
  # exactly `changes` (settings keys => stored values), and leave identical effects.
  def assert_same_save(make, form:, args:, changes:, param_key: 'bots_dca_multi_asset')
    web = make.call
    api = make.call
    before = effects(web)
    assert_equal before.except(:composition), effects(api).except(:composition), 'the twins must start identical'

    web_side = recorded(web) { patch bot_path(id: web.id), params: { param_key => form }, as: :turbo_stream }
    assert_response :success
    assert_nil flash[:alert]
    result = nil
    api_side = recorded(api) { result = BotApi::Bots::UpdateSettings.call(user: @user, bot_id: api.id, **args) }
    assert result.success?, result.error_message

    web_after = effects(web)
    api_after = effects(api)
    changed = web_after[:settings].reject { |key, value| before[:settings].key?(key) && before[:settings][key] == value }
    assert_equal changes.stringify_keys, changed, 'the page must store exactly the expected change'
    assert_equal web_after, api_after, 'the API must leave exactly what the page leaves'
    assert_equal web_side, api_side, 'the same jobs and broadcasts'
    [web, api, result]
  end

  # ---- basket, buying ----

  test 'amount and interval' do
    assert_same_save(-> { basket }, form: { quote_amount: '250', interval: 'week' }, args: { quote_amount: 250, interval: 'week' },
                                    changes: { quote_amount: 250.0, interval: 'week' })
  end

  test 'limit orders on, with the distance in percent, and off again' do
    assert_same_save(-> { basket }, form: { limit_ordered: '1', limit_order_pcnt_distance: '0.5' },
                                    args: { limit_ordered: true, limit_order_pcnt_distance: 0.5 },
                                    changes: { limit_ordered: true, limit_order_pcnt_distance: 0.005 })
    assert_same_save(-> { basket(limit_ordered: true) }, form: { limit_ordered: '0' }, args: { limit_ordered: false },
                                                         changes: { limit_ordered: false })
  end

  test 'Smart Intervals' do
    assert_same_save(-> { basket }, form: { smart_intervaled: '1', smart_interval_quote_amount: '20' },
                                    args: { smart_intervaled: true, smart_interval_quote_amount: 20 },
                                    changes: { smart_intervaled: true, smart_interval_quote_amount: 20.0 })
  end

  test 'starting time on a weekday, on a date, and switched off' do
    assert_same_save(-> { basket }, form: { start_time_enabled: '1', start_time_mode: 'monday', start_time_of_day: '09:30' },
                                    args: { start_time_enabled: true, start_time_mode: 'monday', start_time_of_day: '09:30' },
                                    changes: { start_time_enabled: true, start_time_mode: 'monday', start_time_of_day: '09:30' })
    assert_same_save(-> { basket }, form: { start_time_enabled: '1', start_time_mode: 'date', start_at: '2026-12-01T10:00' },
                                    args: { start_time_enabled: true, start_time_mode: 'date', start_at: '2026-12-01T10:00' },
                                    changes: { start_time_enabled: true, start_time_mode: 'date', start_at: '2026-12-01T10:00:00Z' })
    assert_same_save(-> { basket(start_time_enabled: true, start_time_mode: 'hour', start_time_of_day: '08:00') },
                     form: { start_time_enabled: '0' }, args: { start_time_enabled: false }, changes: { start_time_enabled: false })
  end

  test 'spending cap and rebalancing' do
    assert_same_save(-> { basket }, form: { quote_amount_limited: '1', quote_amount_limit: '500', rebalance_enabled: '1', rebalance_threshold: '5' },
                                    args: { quote_amount_limited: true, quote_amount_limit: 500, rebalance_enabled: true, rebalance_threshold: 5 },
                                    changes: { quote_amount_limited: true, quote_amount_limit: 500.0, rebalance_enabled: true,
                                               rebalance_threshold: 0.05 })
  end

  test 'the four trading conditions' do
    eth = ticker(@eth).id
    assert_same_save(-> { basket },
                     form: { price_limited: '1', price_limit_mode: 'start', price_limit_value_condition: 'above', price_limit: '50000',
                             price_limit_in_ticker_id: eth.to_s },
                     args: { price_limited: true, price_limit_mode: 'start', price_limit_value_condition: 'above', price_limit: 50_000,
                             price_limit_in_ticker_id: eth },
                     changes: { price_limited: true, price_limit_value_condition: 'above', price_limit: 50_000.0, price_limit_in_ticker_id: eth,
                                price_limit_action: 'pause', price_limit_timing_condition: 'after' })
    assert_same_save(-> { basket },
                     form: { price_drop_limited: '1', price_drop_limit_mode: 'flip', price_drop_limit: '10',
                             price_drop_limit_time_window_condition: 'twenty_four_hours', price_drop_limit_in_ticker_id: eth.to_s },
                     args: { price_drop_limited: true, price_drop_limit_mode: 'flip', price_drop_limit: 10,
                             price_drop_limit_time_window_condition: 'twenty_four_hours', price_drop_limit_in_ticker_id: eth },
                     changes: { price_drop_limited: true, price_drop_limit: 0.1, price_drop_limit_time_window_condition: 'twenty_four_hours',
                                price_drop_limit_in_ticker_id: eth, price_drop_limit_action: 'start_selling' })
    assert_same_save(-> { basket },
                     form: { moving_average_limited: '1', moving_average_limit_mode: 'start', moving_average_limit_value_condition: 'above',
                             moving_average_limit_in_ma_type: 'ema', moving_average_limit_in_timeframe: 'four_hours',
                             moving_average_limit_in_period: '50', moving_average_limit_in_ticker_id: eth.to_s },
                     args: { moving_average_limited: true, moving_average_limit_mode: 'start', moving_average_limit_value_condition: 'above',
                             moving_average_limit_in_ma_type: 'ema', moving_average_limit_in_timeframe: 'four_hours',
                             moving_average_limit_in_period: 50, moving_average_limit_in_ticker_id: eth },
                     changes: { moving_average_limited: true, moving_average_limit_value_condition: 'above', moving_average_limit_in_ma_type: 'ema',
                                moving_average_limit_in_timeframe: 'four_hours', moving_average_limit_in_period: 50,
                                moving_average_limit_in_ticker_id: eth, moving_average_limit_action: 'pause',
                                moving_average_limit_timing_condition: 'after' })
    assert_same_save(-> { basket },
                     form: { indicator_limited: '1', indicator_limit_mode: 'start', indicator_limit_value_condition: 'above', indicator_limit: '70',
                             indicator_limit_in_timeframe: 'four_hours', indicator_limit_in_ticker_id: eth.to_s },
                     args: { indicator_limited: true, indicator_limit_mode: 'start', indicator_limit_value_condition: 'above', indicator_limit: 70,
                             indicator_limit_in_timeframe: 'four_hours', indicator_limit_in_ticker_id: eth },
                     changes: { indicator_limited: true, indicator_limit_value_condition: 'above', indicator_limit: 70.0,
                                indicator_limit_in_timeframe: 'four_hours', indicator_limit_in_ticker_id: eth, indicator_limit_action: 'pause',
                                indicator_limit_timing_condition: 'after' })
  end

  # ---- basket composition ----

  test 'weights for some members save unbalanced, the rest at their slider value, and the bot will not start' do
    web, api, = assert_same_save(-> { basket }, form: { allocations: { @btc.id.to_s => '70', @eth.id.to_s => '50.0' } },
                                                args: { allocations: 'BTC:70' },
                                                changes: { allocations: { @btc.id.to_s => 0.7, @eth.id.to_s => 0.5 } })
    [web, api].each { |bot| assert_not bot.valid?(:start), 'an off-100 basket is refused at start, as on the page' }
  end

  test 'weights on a market-cap basket keep its weighting' do
    make = -> { basket(weighting: 'market_cap') }
    assert_same_save(make, form: { allocations: { @btc.id.to_s => '60', @eth.id.to_s => '40' } }, args: { allocations: 'BTC:60,ETH:40' },
                           changes: { allocations: { @btc.id.to_s => 0.6, @eth.id.to_s => 0.4 } })
  end

  test 'market-cap weighting' do
    assert_same_save(-> { basket }, form: { weighting: 'market_cap' }, args: { weighting: 'market_cap' }, changes: { weighting: 'market_cap' })
  end

  test 'adding, removing and normalizing members' do
    assert_same_save(-> { basket }, form: { add_asset_id: @sol.id.to_s }, args: { add_asset_id: @sol.id },
                                    changes: { allocations: { @btc.id.to_s => 0.5, @eth.id.to_s => 0.5, @sol.id.to_s => 0.0 } })
    assert_same_save(-> { basket }, form: { remove_asset_id: @eth.id.to_s }, args: { remove_asset_id: @eth.id },
                                    changes: { allocations: { @btc.id.to_s => 1.0 } })
    unbalanced = -> { basket({ allocations: { @btc.id.to_s => 0.6, @eth.id.to_s => 0.6 } }) }
    assert_same_save(unbalanced, form: { normalize_allocations: '1' }, args: { normalize_allocations: true },
                                 changes: { allocations: { @btc.id.to_s => 0.5, @eth.id.to_s => 0.5 } })
  end

  test 'label' do
    web, api, = assert_same_save(-> { basket }, form: { label: 'Renamed' }, args: { label: 'Renamed' }, changes: {})
    assert_equal(['Renamed'] * 2, [web, api].map { |bot| bot.reload.label })
  end

  # ---- basket, selling ----

  test 'selling amounts and interval' do
    selling = -> { basket(direction: 'selling') }
    assert_same_save(selling, form: { sell_quote_amount: '25', sell_interval: 'week' }, args: { sell_quote_amount: 25, sell_interval: 'week' },
                              changes: { sell_quote_amount: 25.0, sell_interval: 'week' })
    one = -> { basket({ direction: 'selling', sell_denomination: 'base' }, assets: [@btc]) }
    assert_same_save(one, form: { sell_amount: '0.5', base_amount_limited: '1', base_amount_limit: '2' },
                          args: { sell_amount: 0.5, base_amount_limited: true, base_amount_limit: 2 },
                          changes: { sell_amount: 0.5, base_amount_limited: true, base_amount_limit: 2.0 })
  end

  test 'a sell-side condition' do
    eth = ticker(@eth).id
    assert_same_save(-> { basket(direction: 'selling') },
                     form: { sell_price_limited: '1', sell_price_limit_mode: 'flip', sell_price_limit_value_condition: 'above',
                             sell_price_limit: '90000', sell_price_limit_in_ticker_id: eth.to_s },
                     args: { sell_price_limited: true, sell_price_limit_mode: 'flip', sell_price_limit_value_condition: 'above',
                             sell_price_limit: 90_000, sell_price_limit_in_ticker_id: eth },
                     changes: { sell_price_limited: true, sell_price_limit_value_condition: 'above', sell_price_limit: 90_000.0,
                                sell_price_limit_in_ticker_id: eth, sell_price_limit_action: 'start_buying',
                                sell_price_limit_timing_condition: 'after' })
  end

  # ---- the four fields the page can blank: omitted, null and populated ----

  test 'omitted leaves a blankable field, null clears it, a value sets it' do
    start = { direction: 'selling', sell_amount: 0.5, sell_quote_amount: 25.0, rebalance_threshold: 0.05,
              start_at: '2026-12-01T10:00:00Z' }
    make = -> { basket(start) }
    { sell_amount: [0.75, 0.75], sell_quote_amount: [30, 30.0], rebalance_threshold: [7, 0.07],
      start_at: ['2026-12-02T10:00', '2026-12-02T10:00:00Z'] }.each do |name, (value, stored)|
      # Omitted: another field changes, this one stays.
      assert_same_save(make, form: { label: 'Other' }, args: { label: 'Other' }, changes: {})
      # Null: the page posts a blank.
      assert_same_save(make, form: { name => '' }, args: { name => nil }, changes: { name => nil })
      assert_same_save(make, form: { name => value.to_s }, args: { name => value }, changes: { name => stored })
    end
  end

  # ---- index ----

  test 'index knobs, rebalancing, Smart Intervals and starting time' do
    make = -> { index_bot }
    assert_same_save(make, form: { num_coins: '5', allocation_flattening: '0.3' }, args: { num_coins: 5, allocation_flattening: 0.3 },
                           changes: { num_coins: 5, hold_all: false, allocation_flattening: 0.3 }, param_key: 'bots_dca_index')
    assert_same_save(make, form: { rebalance_enabled: '1', rebalance_threshold: '10', limit_ordered: '1', limit_order_pcnt_distance: '0.2' },
                           args: { rebalance_enabled: true, rebalance_threshold: 10, limit_ordered: true, limit_order_pcnt_distance: 0.2 },
                           changes: { rebalance_enabled: true, rebalance_threshold: 0.1, limit_ordered: true, limit_order_pcnt_distance: 0.002 },
                           param_key: 'bots_dca_index')
    assert_same_save(make, form: { start_time_enabled: '1', start_time_mode: 'hour', start_time_of_day: '14:00' },
                           args: { start_time_enabled: true, start_time_mode: 'hour', start_time_of_day: '14:00' },
                           changes: { start_time_enabled: true, start_time_mode: 'hour', start_time_of_day: '14:00' },
                           param_key: 'bots_dca_index')
  end

  # ---- refusals ----

  test 'a field the bot type has no page for is refused, and nothing is written' do
    bot = index_bot
    before = effects(bot)
    result = BotApi::Bots::UpdateSettings.call(user: @user, bot_id: bot.id, price_limited: true)
    assert_equal 'unsupported_setting', result.error_code
    assert_equal 'price_limited applies to basket bots only.', result.error_message
    assert_equal before, effects(bot)

    result = BotApi::Bots::UpdateSettings.call(user: @user, bot_id: basket.id, price_limit: 'abc')
    assert_equal 'invalid_number', result.error_code
  end

  test 'Hyperliquid keeps limit orders on whatever is sent, as the page does' do
    hyperliquid = create(:hyperliquid_exchange)
    [@btc, @eth].each { |asset| create(:ticker, exchange: hyperliquid, base_asset: asset, quote_asset: @usd) }
    bot = basket(exchange: hyperliquid, with_api_key: false)
    result = BotApi::Bots::UpdateSettings.call(user: @user, bot_id: bot.id, limit_ordered: false)
    assert result.success?, result.error_message
    assert_equal true, bot.reload.limit_ordered
  end

  test 'the answer lists the accepted settings in catalogue order' do
    result = BotApi::Bots::UpdateSettings.call(user: @user, bot_id: basket.id, rebalance_enabled: true, label: 'L', quote_amount: 10)
    assert_equal %w[quote_amount label rebalance_enabled], result.data[:updated]
  end
end
