require 'test_helper'

# Every form that takes a key says which permissions the check found missing or unwanted, instead
# of the generic "incorrect permissions" that sends the user back to guess.
class ApiKeyPermissionFormsTest < ActionDispatch::IntegrationTest
  TURBO_STREAM = { 'Accept' => 'text/vnd.turbo-stream.html, text/html' }.freeze

  setup do
    MarketData.stubs(:configured?).returns(true)
    @user = create(:user, admin: true, setup_completed: true)
    @kraken = create(:kraken_exchange)
    Exchanges::Kraken.any_instance.stubs(:set_client)
    ApiKey.any_instance.stubs(:get_validity)
          .returns(Result::Success.new({ missing_permissions: %w[query-ledger], forbidden_permissions: [] }))
    sign_in @user
  end

  def assert_names_the_permission
    assert_response :unprocessable_entity
    assert_includes CGI.unescapeHTML(response.body), 'This key is missing: Data → Query ledger entries.'
  end

  test 'the tracker form' do
    post tracker_add_api_key_path, params: { exchange_id: @kraken.id, api_key: { key: 'k', secret: 's' } },
                                   headers: TURBO_STREAM

    assert_names_the_permission
  end

  test 'the form on an existing bot' do
    bot = create(:dca_single_asset, user: @user, exchange: @kraken, with_api_key: false)

    post bot_add_api_key_path(bot_id: bot.id), params: { api_key: { key: 'k', secret: 's' } }, headers: TURBO_STREAM

    assert_names_the_permission
  end

  test 'the withdrawal rule form' do
    ApiKey.any_instance.stubs(:get_validity)
          .returns(Result::Success.new({ missing_permissions: [], forbidden_permissions: %w[modify-trades] }))

    post rules_withdrawals_add_api_key_path(exchange_id: @kraken.id), params: { api_key: { key: 'k', secret: 's' } },
                                                                      headers: TURBO_STREAM

    assert_response :unprocessable_entity
    assert_includes CGI.unescapeHTML(response.body), 'turn off: Order and Trades → Create & modify orders'
  end
end
