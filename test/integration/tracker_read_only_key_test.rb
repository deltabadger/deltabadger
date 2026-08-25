require 'test_helper'

# What the tracker asks for, and what it reads with. It only ever reads, so a reading key is all it
# has ever needed — and a trading key satisfies it too, because trade permission contains read
# permission. Which means the user is asked for a key at most once per venue, for the smallest one
# that works, and never asked again because a bot already connected that venue.
class TrackerReadOnlyKeyTest < ActionDispatch::IntegrationTest
  setup do
    Tax::EcbFxRates.stubs(:ensure_loaded!)
    Rails.stubs(:cache).returns(ActiveSupport::Cache::MemoryStore.new)
    @user = create(:user, admin: true, setup_completed: true)
    @binance = create(:binance_exchange)
    sign_in @user
  end

  def connect(exchange: @binance)
    Exchanges::Binance.any_instance.stubs(:set_client)
    Exchanges::Binance.any_instance.stubs(:get_balances).returns(Result::Success.new({}))
    AccountTransaction::SyncJob.stubs(:perform_later)
    post tracker_add_api_key_path,
         params: { exchange_id: exchange.id, api_key: { key: 'k-123', secret: 's-456' } }
  end

  test 'a venue with no key is connected with a reading key' do
    connect

    assert_predicate @user.api_keys.sole, :read_only?
    assert_predicate @user.api_keys.sole, :correct?
  end

  test 'a venue a bot already connected is not asked for anything' do
    create(:api_key, user: @user, exchange: @binance, key_type: :trading, status: :correct)

    get new_tracker_add_api_key_path(exchange_id: @binance.id)

    assert_redirected_to tracker_path
  end

  # The case this was built for: a venue that will no longer issue the trading key the bots need.
  # The dead row stays where it is — the bots are entitled to keep failing on it — and the tracker
  # connects beside it rather than through it.
  test 'a venue whose trading key died is connected with a reading key, and keeps the dead one' do
    dead = create(:api_key, user: @user, exchange: @binance, key_type: :trading, status: :incorrect)

    connect

    assert_predicate @user.api_keys.read_only.sole, :correct?
    assert_predicate dead.reload, :incorrect?
  end

  test 'the page reads with a reading key alone' do
    create(:api_key, user: @user, exchange: @binance, key_type: :read_only, status: :correct)

    get tracker_path

    assert_response :success
    assert_select '.tracker'
  end

  test 'sync runs on a reading key' do
    key = create(:api_key, user: @user, exchange: @binance, key_type: :read_only, status: :correct)
    AccountBalance::SyncJob.stubs(:perform_later)
    AccountTransaction::SyncTrackerJob.expects(:perform_later).with(@user.id, [key.id]).once

    post sync_tracker_path

    assert_response :success
  end

  test 'a venue holding both keys syncs once, with the trading one' do
    trading = create(:api_key, user: @user, exchange: @binance, key_type: :trading, status: :correct)
    create(:api_key, user: @user, exchange: @binance, key_type: :read_only, status: :correct)
    AccountBalance::SyncJob.stubs(:perform_later)
    # Two keys on one account would import every row the venue gives no id for twice.
    AccountTransaction::SyncTrackerJob.expects(:perform_later).with(@user.id, [trading.id]).once

    post sync_tracker_path

    assert_response :success
  end
end
