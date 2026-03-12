require 'test_helper'

class ListExchangesToolTest < ActiveSupport::TestCase
  setup do
    @user = create(:user, admin: true)
    ActionMCP::Current.stubs(:user).returns(@user)
  end

  test 'lists connected exchanges' do
    exchange = create(:binance_exchange)
    create(:api_key, user: @user, exchange: exchange, status: :correct)

    response = ListExchangesTool.call
    text = response.contents.first.text

    assert_match(/Connected Exchanges \(1\)/, text)
    assert_match(/Binance/, text)
    assert_match(/correct/, text)
  end

  test 'returns empty message when no exchanges connected' do
    response = ListExchangesTool.call
    text = response.contents.first.text

    assert_equal 'No exchanges connected. Add an API key when creating a bot.', text
  end
end
