require 'test_helper'

# Issue #153 part two: the permissions a key needs must be readable from the API-keys screen, not
# only while pasting a new key.
class Settings::ApiKeyPermissionsTest < ActionDispatch::IntegrationTest
  include Devise::Test::IntegrationHelpers

  setup do
    create(:user, admin: true, setup_completed: true) # platform requires an admin to exist
    @user = create(:user, time_zone: 'UTC', locale: 'en')
    @kraken = create(:kraken_exchange)
    sign_in @user
  end

  test 'a trading key shows the setup instructions, ledger permission included' do
    api_key = create(:api_key, user: @user, exchange: @kraken)

    get settings_api_key_permissions_path(id: api_key.id)

    assert_response :success
    assert_includes @response.body, 'Query ledger entries'
    assert_no_match(/translation_missing/, @response.body)
  end

  test 'a withdrawal key shows the withdrawal instructions, which exist only in English' do
    api_key = create(:api_key, user: @user, exchange: @kraken, key_type: :withdrawal)
    @user.update!(locale: 'pl')

    get settings_api_key_permissions_path(id: api_key.id, locale: 'pl')

    assert_response :success
    assert_no_match(/translation_missing/, @response.body)
  end

  test 'another user API key is not readable' do
    other_key = create(:api_key, user: create(:user), exchange: @kraken)

    get settings_api_key_permissions_path(id: other_key.id)

    assert_response :not_found
  end

  test 'the settings tag links to the permissions of an exchange that has instructions' do
    api_key = create(:api_key, user: @user, exchange: @kraken)

    get settings_connect_path

    assert_response :success
    assert_includes @response.body, settings_api_key_permissions_path(id: api_key.id)
  end

  # render_instructions_from returns nil for these, and a link to an empty modal is worse than
  # no link at all.
  test 'an exchange with no instructions is not linked' do
    ibkr = create(:ibkr_exchange)
    api_key = create(:api_key, user: @user, exchange: ibkr)

    get settings_connect_path

    assert_response :success
    assert_not_includes @response.body, settings_api_key_permissions_path(id: api_key.id)
    assert_includes @response.body, ibkr.name
  end
end
