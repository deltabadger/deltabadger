require 'test_helper'

class ListBotsToolTest < ActiveSupport::TestCase
  setup do
    @user = create(:user, admin: true)
    ActionMCP::Current.stubs(:user).returns(@user)
  end

  test 'lists bots with status, type, pair, and exchange' do
    create(:dca_single_asset, user: @user, status: :scheduled, started_at: Time.current)

    response = ListBotsTool.call
    text = response.contents.first.text

    assert_match(/Bots \(1\)/, text)
    assert_match(/Dca Single Asset/, text)
    assert_match(%r{BTC/USD}, text)
    assert_match(/Binance/, text)
    assert_match(/scheduled/, text)
  end

  test 'filters by status' do
    btc = create(:asset, :bitcoin)
    eth = create(:asset, :ethereum)
    usd = create(:asset, :usd)
    exchange = create(:binance_exchange)
    create(:dca_single_asset, user: @user, status: :scheduled, started_at: Time.current,
                              base_asset: btc, quote_asset: usd, exchange: exchange)
    create(:dca_single_asset, :stopped, user: @user, base_asset: eth, quote_asset: usd,
                                        exchange: exchange)

    response = ListBotsTool.call('status' => 'scheduled')
    text = response.contents.first.text

    assert_match(/Bots \(1\)/, text)
    assert_match(/scheduled/, text)
    assert_no_match(/stopped/, text)
  end

  test 'excludes deleted bots' do
    create(:dca_single_asset, :deleted, user: @user)

    response = ListBotsTool.call
    text = response.contents.first.text

    assert_equal 'No bots found.', text
  end

  test 'returns empty message when no bots' do
    response = ListBotsTool.call
    text = response.contents.first.text

    assert_equal 'No bots found.', text
  end

  test 'shows dual asset bot pair format' do
    create(:dca_dual_asset, user: @user, status: :scheduled, started_at: Time.current)

    response = ListBotsTool.call
    text = response.contents.first.text

    assert_match(%r{BTC\+ETH/USD}, text)
    assert_match(/Dca Dual Asset/, text)
  end
end
