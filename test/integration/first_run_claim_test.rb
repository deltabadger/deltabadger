require 'test_helper'

class FirstRunClaimTest < ActionDispatch::IntegrationTest
  test 'docker claim reaches the Index Portfolio picker without the CoinGecko step' do
    launchpad = mock('launchpad_client')
    Clients::Launchpad.stubs(:new).returns(launchpad)
    launchpad.expects(:claim).with('dbc_first_run').returns(Result::Success.new(claim_payload))
    Setup::SeedAndSyncJob.expects(:perform_later).once

    with_env('CLAIM_TOKEN', 'dbc_first_run') do
      get new_setup_path
    end

    assert_response :success
    assert_select 'input#user_name[value=?]', 'Owner'
    assert_select 'input#user_email[value=?]', 'owner@example.com'

    post setup_path, params: {
      user: { name: 'Owner', email: 'owner@example.com', password: 'SecurePass1!' }
    }
    assert_redirected_to bots_path
    follow_redirect!
    assert_response :success

    get new_bot_path
    assert_response :success
    assert_select "a[href='#{new_bots_dca_indexes_setup_coingecko_path}']"

    get new_bots_dca_indexes_setup_coingecko_path
    assert_redirected_to new_bots_dca_indexes_pick_index_path
  end

  private

  def claim_payload
    {
      identity: { email: 'owner@example.com', name: 'Owner' },
      market_data: { url: 'https://market-data.example.com', token: 'dbi_token' },
      proxies: {}
    }
  end

  def with_env(key, value)
    original = ENV[key]
    ENV[key] = value
    yield
  ensure
    original.nil? ? ENV.delete(key) : ENV[key] = original
  end
end
