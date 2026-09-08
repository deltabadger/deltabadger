require 'test_helper'

# The Sell confirmation is the one seam where the answer changes the outcome of the very action being
# taken: this sale can start a window, so the decision is recorded before the order is enqueued,
# never after. A modal appended to the response would arrive too late to protect this sale.
class Bots::LiquidationWashSaleTest < ActionDispatch::IntegrationTest
  def setup
    create(:user, admin: true, setup_completed: true) # onboarding gate
    @user = create(:user)
    @bot = create(:dca_index, user: @user)
    sign_in @user
    sellable_quitter
  end

  test 'an undecided account is asked inside the confirmation, not in a modal over it' do
    get new_bot_liquidation_path(bot_id: @bot.id, symbol: 'CCC')

    assert_response :success
    assert_select '.wash-sale-question'
    assert_select "input[name='wash_sale[enabled]']", count: 2
  end

  test 'a decided account gets the plain confirmation' do
    @user.update!(wash_sale_enabled: false)

    get new_bot_liquidation_path(bot_id: @bot.id, symbol: 'CCC')

    assert_select '.wash-sale-question', count: 0
  end

  test 'an unanswered question refuses the sale rather than selling first and asking later' do
    post bot_liquidation_path(bot_id: @bot.id, symbol: 'CCC')

    assert_response :unprocessable_entity
    assert_not_predicate queued(Bot::LiquidateExitedJob), :exists?
    assert_not_predicate @user.reload, :wash_sale_decided?
  end

  test 'the answer is recorded before the sale is enqueued' do
    post bot_liquidation_path(bot_id: @bot.id, symbol: 'CCC'),
         params: { wash_sale: { enabled: '1', jurisdiction: 'IE' } }

    @user.reload
    assert_equal 28, @user.wash_sale_days, 'the window this sale may need is already in place'
    assert_predicate queued(Bot::LiquidateExitedJob), :exists?
  end

  test 'declining sells too — the question is about the rule, not about permission' do
    post bot_liquidation_path(bot_id: @bot.id, symbol: 'CCC'), params: { wash_sale: { enabled: '0' } }

    assert_predicate @user.reload, :wash_sale_decided?
    assert_not_predicate @user, :wash_sale_enabled?
    assert_predicate queued(Bot::LiquidateExitedJob), :exists?
  end

  test 'a decided account is not asked again on the next sale' do
    @user.update!(wash_sale_enabled: false)

    post bot_liquidation_path(bot_id: @bot.id, symbol: 'CCC')

    assert_predicate queued(Bot::LiquidateExitedJob), :exists?
  end

  # Solid Queue is the adapter here, so ActiveJob's test-adapter assertions are unavailable.
  def queued(job_class)
    SolidQueue::Job.where(class_name: job_class.name)
  end

  def sellable_quitter
    { 'AAA' => :in_index, 'CCC' => :exited }.each do |symbol, role|
      asset = create(:asset, symbol: symbol, name: "Coin #{symbol}", external_id: "coin-#{symbol.downcase}")
      ticker = create(:ticker, exchange: @bot.exchange, base_asset: asset, quote_asset: @bot.quote_asset)
      BotIndexAsset.create!(bot: @bot, asset: asset, ticker: ticker, in_index: role == :in_index,
                            target_allocation: role == :in_index ? 1.0 : nil,
                            entered_at: Time.current, exited_at: role == :in_index ? nil : Time.current)
    end
    warm_prices('AAA' => 100, 'CCC' => 20)
  end

  def warm_prices(values)
    Rails.stubs(:cache).returns(ActiveSupport::Cache::MemoryStore.new)
    data = @bot.metrics.deep_dup
    data[:asset_values] = values.transform_values do |v|
      { amount: v.to_d / 100, quote_invested: v.to_d, current_value: v.to_d,
        current_price: 100, avg_price: 100, pnl_percentage: 0 }
    end
    data[:asset_breakdown] = values.transform_values { |v| { amount: v.to_d / 100, quote_invested: v.to_d } }
    Rails.cache.write(@bot.send(:metrics_cache_key), data)
    Rails.cache.write(@bot.send(:metrics_with_current_prices_cache_key), data)
  end
end
