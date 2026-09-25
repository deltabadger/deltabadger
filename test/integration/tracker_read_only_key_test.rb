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

  # When the key the tracker reads with cannot read, the user chooses: replace the trading key (fixes
  # bots and tracker, needs every trading permission) or add a tracker key (read-only, bots untouched).
  # Nothing picks for them: an untyped request only ever works on the tracker's own slot.
  class KeyChoiceTest < ActionDispatch::IntegrationTest
    setup do
      MarketData.stubs(:configured?).returns(true)
      Tax::EcbFxRates.stubs(:ensure_loaded!)
      Rails.stubs(:cache).returns(ActiveSupport::Cache::MemoryStore.new)
      @user = create(:user, admin: true, setup_completed: true)
      @kraken = create(:kraken_exchange)
      @trading = create(:api_key, user: @user, exchange: @kraken, key_type: :trading, status: :correct,
                                  last_sync_error: 'EGeneral:Permission denied')
      @stored = @trading.key
      Exchanges::Kraken.any_instance.stubs(:set_client)
      sign_in @user
    end

    def open_form(key_type: nil)
      get new_tracker_add_api_key_path(exchange_id: @kraken.id, key_type:), headers: { 'Turbo-Frame' => 'modal' }
    end

    def submit(key_type: nil, valid: true, exchange: @kraken)
      Exchanges::Kraken.any_instance.stubs(:get_api_key_validity).returns(Result::Success.new(valid))
      Exchanges::Kraken.any_instance.stubs(:get_read_api_key_validity).returns(Result::Success.new(valid))
      post tracker_add_api_key_path(exchange_id: exchange.id, key_type:),
           params: { api_key: { key: 'new-key', secret: 'new-secret' } }
    end

    def assert_trading_untouched
      @trading.reload
      assert_equal @stored, @trading.key
      assert_predicate @trading, :correct?
    end

    # --- which form ------------------------------------------------------------------------------

    test 'untyped, a trading key missing a permission gets the tracker key form' do
      open_form

      assert_response :success
      assert_select 'form[action=?]', tracker_add_api_key_path(exchange_id: @kraken.id, key_type: 'read_only')
      assert_includes response.body, 'the tracker only reads'
    end

    test 'untyped, a trading key with any other sync error still redirects' do
      @trading.update_column(:last_sync_error, 'EAPI:Rate limit exceeded')

      get new_tracker_add_api_key_path(exchange_id: @kraken.id)

      assert_redirected_to tracker_path
    end

    test 'an unknown or withdrawal key type is treated as untyped' do
      open_form(key_type: 'withdrawal')

      assert_select 'form[action=?]', tracker_add_api_key_path(exchange_id: @kraken.id, key_type: 'read_only')
    end

    test 'Replace trading key opens the trading form, with the trading steps' do
      open_form(key_type: 'trading')

      assert_select 'form[action=?]', tracker_add_api_key_path(exchange_id: @kraken.id, key_type: 'trading')
      assert_includes response.body, 'Create & modify orders'
    end

    test 'an explicit replacement still opens even if the stored key checks out again' do
      @trading.update_columns(status: ApiKey.statuses[:incorrect])
      ApiKey.any_instance.stubs(:get_validity).returns(Result::Success.new(true))

      open_form(key_type: 'trading')

      assert_response :success
      assert_select 'form[action=?]', tracker_add_api_key_path(exchange_id: @kraken.id, key_type: 'trading')
    end

    test 'the form never echoes the stored credentials' do
      open_form(key_type: 'trading')

      assert_not_includes response.body, @trading.key
      assert_not_includes response.body, @trading.secret
    end

    # The session names the exchange of the LAST form opened; the form carries its own.
    test 'a form submits to the exchange it was opened for, whatever another tab opened since' do
      open_form(key_type: 'read_only')
      binance = create(:binance_exchange)
      get new_tracker_add_api_key_path(exchange_id: binance.id), headers: { 'Turbo-Frame' => 'modal' }
      AccountTransaction::SyncJob.stubs(:perform_later)

      submit(key_type: 'read_only')

      assert_equal @kraken, @user.api_keys.read_only.sole.exchange
    end

    # --- Replace trading key -------------------------------------------------------------------

    test 'Replace trading key writes into the trading row, and bots keep running' do
      bot = create(:dca_single_asset, user: @user, exchange: @kraken, status: :scheduled)
      AccountTransaction::SyncJob.expects(:perform_later).with(@trading).once

      submit(key_type: 'trading')

      @trading.reload
      assert_equal 'new-key', @trading.key
      assert_predicate @trading, :correct?
      assert_nil @trading.last_sync_error
      assert_equal [@trading.id], @user.api_keys.pluck(:id)
      assert_predicate bot.reload, :scheduled?
      assert_includes response.body, '<turbo-stream action="update" target="sync-warnings">'
    end

    test 'a trading replacement the venue rejects, or cannot check, leaves the key as it was' do
      AccountTransaction::SyncJob.expects(:perform_later).never

      submit(key_type: 'trading', valid: false)
      assert_response :unprocessable_entity
      assert_trading_untouched

      Exchanges::Kraken.any_instance.stubs(:get_api_key_validity).returns(Result::Failure.new('EService:Unavailable'))
      post tracker_add_api_key_path(exchange_id: @kraken.id, key_type: 'trading'),
           params: { api_key: { key: 'new-key', secret: 'new-secret' } }
      assert_response :unprocessable_entity
      assert_trading_untouched
    end

    # --- Add tracker key -----------------------------------------------------------------------

    test 'Add tracker key puts a reading key beside the trading key, and the tracker reads with it' do
      bot = create(:dca_single_asset, user: @user, exchange: @kraken, status: :scheduled)
      AccountTransaction::SyncJob.expects(:perform_later).with(&:read_only?).once

      submit(key_type: 'read_only')

      reading = @user.api_keys.read_only.sole
      assert_predicate reading, :correct?
      assert_trading_untouched
      assert_predicate bot.reload, :scheduled?
      assert_equal [reading], ApiKey.reading(@user.api_keys.includes(:exchange))
    end

    test 'an untyped submission never writes the trading row' do
      AccountTransaction::SyncJob.stubs(:perform_later)

      submit

      assert_trading_untouched
      assert_predicate @user.api_keys.read_only.sole, :correct?
    end

    # --- the banner ----------------------------------------------------------------------------

    def banner
      get tracker_path
      css_select('#sync-warnings').first&.to_html.to_s
    end

    test 'a trading key missing a permission offers both, and says what is wrong' do
      html = CGI.unescapeHTML(banner)

      assert_includes html, 'the tracker reads with your trading key, but it is missing a permission'
      assert_includes html, new_tracker_add_api_key_path(exchange_id: @kraken.id, key_type: 'trading')
      assert_includes html, new_tracker_add_api_key_path(exchange_id: @kraken.id, key_type: 'read_only')
    end

    test 'a dead trading key says so, and offers both' do
      @trading.update_columns(status: ApiKey.statuses[:incorrect], last_sync_error: 'EAPI:Invalid key')
      html = CGI.unescapeHTML(banner)

      assert_includes html, 'your trading key no longer works'
      assert_includes html, new_tracker_add_api_key_path(exchange_id: @kraken.id, key_type: 'trading')
      assert_includes html, new_tracker_add_api_key_path(exchange_id: @kraken.id, key_type: 'read_only')
    end

    test 'a failing tracker key offers only to replace it' do
      create(:api_key, user: @user, exchange: @kraken, key_type: :read_only, status: :correct,
                       last_sync_error: 'EGeneral:Permission denied')
      html = CGI.unescapeHTML(banner)

      assert_includes html, 'your tracker key is missing a permission'
      assert_includes html, new_tracker_add_api_key_path(exchange_id: @kraken.id, key_type: 'read_only')
      assert_not_includes html, new_tracker_add_api_key_path(exchange_id: @kraken.id, key_type: 'trading')
    end

    test 'a rate limit gets no buttons' do
      @trading.update_column(:last_sync_error, 'EAPI:Rate limit exceeded')

      assert_select_in_banner = banner
      assert_not_includes assert_select_in_banner, 'add_api_key'
    end

    test 'a working tracker key hides the trading key it replaced' do
      create(:api_key, user: @user, exchange: @kraken, key_type: :read_only, status: :correct)

      assert_not_includes banner, 'Kraken'
    end

    test 'both keys dead: one fix, for the tracker key' do
      @trading.update_columns(status: ApiKey.statuses[:incorrect], last_sync_error: 'EAPI:Invalid key')
      create(:api_key, user: @user, exchange: @kraken, key_type: :read_only, status: :incorrect,
                       last_sync_error: 'EAPI:Invalid key')
      html = CGI.unescapeHTML(banner)

      assert_equal 1, html.scan('Kraken:').size
      assert_includes html, 'your tracker key no longer works'
    end
  end
end
