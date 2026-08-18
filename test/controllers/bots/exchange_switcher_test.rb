require 'test_helper'

# The exchange switcher was gated on `stopped?`, so a bot that had never been started — the exact
# state a bot sits in right after the wizard, and the state a user lands in with a rejected key —
# got no dropdown at all. Not-working is the gate: `created` and `stopped` both switch freely.
class ExchangeSwitcherTest < ActionDispatch::IntegrationTest
  setup do
    @user = create(:user, admin: true, setup_completed: true)
    sign_in @user

    @base = create(:asset, :bitcoin)
    @quote = create(:asset, :usd)
    @kraken = create(:kraken_exchange)
    create(:ticker, exchange: @kraken, base_asset: @base, quote_asset: @quote)
  end

  def bot_with(status:, with_api_key:)
    create(:dca_single_asset, status, user: @user, base_asset: @base, quote_asset: @quote,
                                      with_api_key:)
  end

  test 'a never-started bot with a disconnected key can still be switched to another exchange' do
    bot = create(:dca_single_asset, user: @user, base_asset: @base, quote_asset: @quote,
                                    with_api_key: false)

    get bot_path(id: bot.id)

    assert_response :success
    assert_match 'dropdown--exchanges', response.body
    assert_match @kraken.name, response.body
  end

  test 'a working bot offers no switcher' do
    bot = bot_with(status: :started, with_api_key: true)

    get bot_path(id: bot.id)

    assert_response :success
    refute_match 'dropdown--exchanges', response.body
  end
end
