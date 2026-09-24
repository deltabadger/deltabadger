require 'test_helper'

# What the tracker asks for, and what it reads with. It only ever reads, so a reading key is all it
# has ever needed — and a trading key satisfies it too, because trade permission contains read
# permission. Which means the user is asked for a key at most once per venue, for the smallest one
# that works, and never asked again because a bot already connected that venue.
class TrackerReadOnlyKeyTest < ActionDispatch::IntegrationTest
  setup do
    # The tracker refuses to render a portfolio without a market-data feed, which every real
    # install has. Stubbed rather than configured: naming a provider would send these tests at a
    # real endpoint, and what they exercise is the page, not the feed.
    MarketData.stubs(:configured?).returns(true)
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

  # A key the venue accepts but that lacks a scope stays :correct — its bots keep trading — so
  # "a bot already connected this venue" is no longer the whole answer: the key the tracker reads
  # with cannot read. The user's fix is a new key, and this step is where it goes.
  class MissingPermissionTest < ActionDispatch::IntegrationTest
    setup do
      MarketData.stubs(:configured?).returns(true)
      Tax::EcbFxRates.stubs(:ensure_loaded!)
      Rails.stubs(:cache).returns(ActiveSupport::Cache::MemoryStore.new)
      @user = create(:user, admin: true, setup_completed: true)
      @kraken = create(:kraken_exchange)
      @key = create(:api_key, user: @user, exchange: @kraken, key_type: :trading, status: :correct,
                              last_sync_error: 'EGeneral:Permission denied')
      Exchanges::Kraken.any_instance.stubs(:set_client)
      sign_in @user
    end

    def replace_with(valid:)
      Exchanges::Kraken.any_instance.stubs(:get_api_key_validity).returns(Result::Success.new(valid))
      post tracker_add_api_key_path,
           params: { exchange_id: @kraken.id, api_key: { key: 'new-key', secret: 'new-secret' } }
    end

    test 'a trading key missing a permission gets its form instead of a redirect' do
      get new_tracker_add_api_key_path(exchange_id: @kraken.id), headers: { 'Turbo-Frame' => 'modal' }

      assert_response :success
      assert_select 'form[action=?]', tracker_add_api_key_path
    end

    # The form is bound to the stored row, and this row is a live trading key.
    test 'the form never echoes the stored credentials' do
      get new_tracker_add_api_key_path(exchange_id: @kraken.id), headers: { 'Turbo-Frame' => 'modal' }

      assert_not_includes response.body, @key.key
      assert_not_includes response.body, @key.secret
    end

    test 'a trading key with any other sync error still redirects' do
      @key.update_column(:last_sync_error, 'EAPI:Rate limit exceeded')

      get new_tracker_add_api_key_path(exchange_id: @kraken.id)

      assert_redirected_to tracker_path
    end

    # Into the same row, so the bots on it carry on with the new credentials and nothing is stopped.
    test 'the new key takes the trading key’s place and syncs' do
      AccountTransaction::SyncJob.expects(:perform_later).with(@key).once

      replace_with(valid: true)

      assert_equal [@key.id], @user.api_keys.pluck(:id)
      @key.reload
      assert_equal 'new-key', @key.key
      assert_predicate @key, :trading?
      assert_predicate @key, :correct?
      assert_nil @key.last_sync_error
    end

    # The row is updated, never destroyed, so none of the key-deletion paths that stop bots runs.
    test 'bots trading on the key keep running through the replacement' do
      bot = create(:dca_single_asset, user: @user, exchange: @kraken, status: :scheduled)
      AccountTransaction::SyncJob.stubs(:perform_later)

      replace_with(valid: true)

      assert_predicate bot.reload, :scheduled?
      assert_equal [@key.id], ApiKey.where(user: @user, exchange: @kraken).pluck(:id)
    end

    # The banner is data-turbo-permanent, so the redirect alone would carry the old warning over.
    test 'a replacement clears the warning in the same response' do
      AccountTransaction::SyncJob.stubs(:perform_later)

      replace_with(valid: true)

      assert_includes response.body, '<turbo-stream action="update" target="sync-warnings">'
      assert_not_includes response.body, new_tracker_add_api_key_path(exchange_id: @kraken.id)
    end

    test 'a venue that cannot be asked leaves the working key untouched' do
      Exchanges::Kraken.any_instance.stubs(:get_api_key_validity).returns(Result::Failure.new('EService:Unavailable'))
      AccountTransaction::SyncJob.expects(:perform_later).never

      post tracker_add_api_key_path,
           params: { exchange_id: @kraken.id, api_key: { key: 'new-key', secret: 'new-secret' } }

      assert_response :unprocessable_entity
      @key.reload
      assert_not_equal 'new-key', @key.key
      assert_predicate @key, :correct?
    end

    test 'a new key the venue rejects leaves the working one untouched' do
      AccountTransaction::SyncJob.expects(:perform_later).never

      replace_with(valid: false)

      assert_response :unprocessable_entity
      @key.reload
      assert_not_equal 'new-key', @key.key
      assert_predicate @key, :correct?
      assert_equal 'EGeneral:Permission denied', @key.last_sync_error
    end

    test 'the tracker banner links to the form' do
      get tracker_path

      assert_select '#sync-warnings a.rbutton[href=?]', new_tracker_add_api_key_path(exchange_id: @kraken.id)
    end
  end
end
