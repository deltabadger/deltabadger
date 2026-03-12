require 'test_helper'

class GetBotDetailsToolTest < ActiveSupport::TestCase
  setup do
    @user = create(:user, admin: true)
    ActionMCP::Current.stubs(:user).returns(@user)
  end

  test 'returns bot details with metrics' do
    bot = create(:dca_single_asset, user: @user, status: :scheduled, started_at: Time.current)
    metrics = {
      total_quote_amount_invested: 500.0,
      total_amount_value_in_quote: 550.0,
      pnl: 0.1,
      average_buy_price: 45_000.0,
      total_base_amount: 0.011
    }
    Bots::DcaSingleAsset.any_instance.stubs(:metrics).returns(metrics)

    response = GetBotDetailsTool.call('bot_id' => bot.id)
    text = response.contents.first.text

    assert_match(/Status: scheduled/, text)
    assert_match(/Binance/, text)
    assert_match(%r{BTC/USD}, text)
    assert_match(/Total invested: 500.0/, text)
    assert_match(/Current value: 550.0/, text)
    assert_match(%r{P/L: \+10.0%}, text)
    assert_match(/Average buy price: 45000.0/, text)
  end

  test 'handles missing bot' do
    response = GetBotDetailsTool.call('bot_id' => 99_999)
    text = response.contents.first.text

    assert_equal 'Bot not found.', text
  end

  test 'does not find deleted bots' do
    bot = create(:dca_single_asset, :deleted, user: @user)

    response = GetBotDetailsTool.call('bot_id' => bot.id)
    text = response.contents.first.text

    assert_equal 'Bot not found.', text
  end

  test 'handles metrics error gracefully' do
    bot = create(:dca_single_asset, user: @user, status: :scheduled, started_at: Time.current)
    Bots::DcaSingleAsset.any_instance.stubs(:metrics).raises(StandardError.new('Cache error'))

    response = GetBotDetailsTool.call('bot_id' => bot.id)
    text = response.contents.first.text

    assert_match(/Bot:/, text)
    assert_match(/Metrics unavailable: Cache error/, text)
  end

  test 'shows bot details without metrics when none available' do
    bot = create(:dca_single_asset, user: @user, status: :created)
    Bots::DcaSingleAsset.any_instance.stubs(:metrics).returns(nil)

    response = GetBotDetailsTool.call('bot_id' => bot.id)
    text = response.contents.first.text

    assert_match(/Bot:/, text)
    assert_match(/Status: created/, text)
    assert_no_match(/Performance/, text)
  end

  test 'shows negative PnL correctly' do
    bot = create(:dca_single_asset, user: @user, status: :scheduled, started_at: Time.current)
    metrics = {
      total_quote_amount_invested: 1000.0,
      total_amount_value_in_quote: 800.0,
      pnl: -0.2,
      average_buy_price: 50_000.0,
      total_base_amount: 0.02
    }
    Bots::DcaSingleAsset.any_instance.stubs(:metrics).returns(metrics)

    response = GetBotDetailsTool.call('bot_id' => bot.id)
    text = response.contents.first.text

    assert_match(%r{P/L: -20.0%}, text)
  end
end
