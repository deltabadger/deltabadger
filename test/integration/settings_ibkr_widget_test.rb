require 'test_helper'

# The IBKR widget only makes sense where an IBKR catalog can exist — with deltabadger
# market data (data-api). Self-hosted containers have no IBKR catalog source (no
# list-all-instruments endpoint, no free market data), so the connect entry point is
# hidden. An existing connection stays visible so it can still be disconnected.
class SettingsIbkrWidgetTest < ActionDispatch::IntegrationTest
  include Devise::Test::IntegrationHelpers

  setup do
    create(:user, admin: true, setup_completed: true)
    @user = create(:user, setup_completed: true)
    @ibkr = create(:ibkr_exchange)
    sign_in @user
  end

  test 'widget shows on hosted — the data API serves the IBKR catalog' do
    MarketDataSettings.stubs(:deltabadger_available?).returns(true)

    get settings_connect_path
    assert_response :success
    assert_select 'a[href=?]', settings_ibkr_connect_path
  end

  test 'widget is hidden on self-hosted — no data API, no IBKR catalog source' do
    get settings_connect_path
    assert_response :success
    assert_select 'a[href=?]', settings_ibkr_connect_path, count: 0
  end

  test 'widget stays visible for a user with an existing IBKR connection' do
    @user.api_keys.create!(exchange: @ibkr, key_type: :trading, status: :correct)

    get settings_connect_path
    assert_response :success
    assert_select 'a[href=?]', settings_ibkr_connect_path
  end

  test 'widget shows PENDING while IBKR is still activating the consumer key' do
    @user.api_keys.create!(exchange: @ibkr, key_type: :trading, status: :pending_activation)

    get settings_connect_path
    assert_response :success
    assert_includes response.body, '>PENDING<'
    refute_includes response.body, 'ACTION NEEDED'
  end

  test 'widget flags a registration that never activated, matching the wizard' do
    key = @user.api_keys.create!(exchange: @ibkr, key_type: :trading, status: :pending_activation)
    key.update_column(:updated_at, ApiKey::ACTIVATION_DEADLINE.ago - 1.day)

    get settings_connect_path
    assert_response :success
    # Leaving this on PENDING would contradict the wizard, which by now says the registration
    # is dead and the user has to start over.
    assert_includes response.body, 'ACTION NEEDED'
    refute_includes response.body, '>PENDING<'
  end
end
