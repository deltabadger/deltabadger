require 'test_helper'

# Market data is instance-wide state. Every write path must be admin-only, or a
# second household account can repoint the whole instance at their own key.
class MarketDataAdminGateTest < ActionDispatch::IntegrationTest
  include Devise::Test::IntegrationHelpers

  setup do
    # The app runs solid_queue in every environment (config/initializers/active_job.rb);
    # assert_no_enqueued_jobs needs the :test adapter to record enqueues in-memory.
    @original_adapter = ActiveJob::Base.queue_adapter
    ActiveJob::Base.queue_adapter = :test

    create(:user, admin: true, setup_completed: true)
    @member = create(:user, setup_completed: true)
    sign_in @member

    # Stub validation to SUCCEED. Without this the controller rejects the key at the
    # CoinGecko call and returns 422 — the test would go red before the fix and green
    # after it while never exercising the authorization gap it exists to prove.
    Coingecko.any_instance.stubs(:get_top_coins_by_market_cap).returns(Result::Success.new([]))
    Coingecko.any_instance.stubs(:get_coins_list_with_market_data).returns(Result::Success.new([]))
  end

  teardown do
    ActiveJob::Base.queue_adapter = @original_adapter
  end

  test 'a non-admin cannot set the CoinGecko key from the tracker' do
    assert_no_changes -> { AppConfig.coingecko_api_key } do
      post '/tracker/setup_coingecko', params: { api_key: 'attacker-key' }
    end
    assert_response :forbidden
  end

  test 'a non-admin cannot set the CoinGecko key from the index wizard' do
    assert_no_changes -> { AppConfig.coingecko_api_key } do
      post '/bots/dca_indexes/setup_coingecko', params: { api_key: 'attacker-key' }
    end
    assert_response :forbidden
  end

  test 'a non-admin cannot enqueue the seed-and-sync job' do
    assert_no_enqueued_jobs only: Setup::SeedAndSyncJob do
      post '/tracker/setup_coingecko', params: { api_key: 'attacker-key' }
    end
  end

  test 'an admin still can' do
    sign_out @member
    sign_in User.find_by(admin: true)

    assert_changes -> { AppConfig.coingecko_api_key } do
      post '/bots/dca_indexes/setup_coingecko', params: { api_key: 'real-key' }
    end
    assert_response :redirect
  end

  test 'a non-admin does not see the CoinGecko key form on the tracker export modal' do
    get '/tracker/export_modal'
    assert_response :success
    assert_select 'input[name="api_key"]', count: 0
  end

  test 'a non-admin does not see the CoinGecko key form on the index wizard' do
    get '/bots/dca_indexes/setup_coingecko/new'
    assert_response :success
    assert_select 'input[name="api_key"]', count: 0
  end
end
