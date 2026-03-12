require 'test_helper'

class GetPortfolioSummaryToolTest < ActiveSupport::TestCase
  setup do
    @user = create(:user, admin: true)
    ActionMCP::Current.stubs(:user).returns(@user)
  end

  test 'returns portfolio summary with global pnl' do
    create(:dca_single_asset, user: @user, status: :scheduled, started_at: Time.current)
    User.any_instance.stubs(:global_pnl).returns({ percent: 0.15, profit_usd: 150.0 })
    Bots::DcaSingleAsset.any_instance.stubs(:metrics).returns({
                                                                pnl: 0.15,
                                                                total_quote_amount_invested: 1000.0,
                                                                total_amount_value_in_quote: 1150.0
                                                              })

    response = GetPortfolioSummaryTool.call
    text = response.contents.first.text

    assert_match(/Portfolio Summary/, text)
    assert_match(/Total bots: 1/, text)
    assert_match(%r{Global P/L: \+15.0%}, text)
    assert_match(/\+\$150.0/, text)
  end

  test 'handles no bots' do
    response = GetPortfolioSummaryTool.call
    text = response.contents.first.text

    assert_equal 'No bots found. Create a bot to start tracking your portfolio.', text
  end

  test 'handles nil global pnl' do
    create(:dca_single_asset, user: @user, status: :scheduled, started_at: Time.current)
    User.any_instance.stubs(:global_pnl).returns(nil)
    Bots::DcaSingleAsset.any_instance.stubs(:metrics).returns(nil)

    response = GetPortfolioSummaryTool.call
    text = response.contents.first.text

    assert_match(/Not available/, text)
    assert_match(/No metrics yet/, text)
  end
end
