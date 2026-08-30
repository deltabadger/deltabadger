require 'test_helper'

# The banner that says a venue's history may be incomplete.
#
# It reads `last_sync_error`, which is a note left ON A KEY and erased only when THAT key syncs
# successfully. That was sound while a venue had one key. It stopped being sound the moment a venue
# could have two: a rejected trading key keeps its note forever, because nothing syncs it any more —
# the tracker reads through the reading key beside it. The page then warns that Binance history is
# missing while showing that same history.
#
# So the question the banner asks is not "does any key have an error" but "did the key we READ WITH
# fail" — and a venue with no working key at all still has to be able to answer yes.
class TrackerSyncBannerTest < ActionDispatch::IntegrationTest
  setup do
    Tax::EcbFxRates.stubs(:ensure_loaded!)
    Rails.stubs(:cache).returns(ActiveSupport::Cache::MemoryStore.new)
    @user = create(:user, admin: true, setup_completed: true)
    @binance = create(:binance_exchange)
    sign_in @user
  end

  def banner_for?(exchange_name)
    get tracker_path
    assigns_failures = css_select('#sync-warnings').text
    assigns_failures.include?(exchange_name)
  end

  test 'a dead trading key stops warning once a reading key took over' do
    create(:api_key, user: @user, exchange: @binance, key_type: :trading, status: :incorrect,
                     last_sync_error: '{"code":-2015,"msg":"Invalid API-key, IP, or permissions for action."}')
    create(:api_key, user: @user, exchange: @binance, key_type: :read_only, status: :correct)

    assert_not banner_for?('Binance'),
               'the venue reads fine through the key beside it'
  end

  # The half that must not regress: a venue with nothing working still says so, or a broken sync
  # goes silent and a tax report is quietly built on a hole.
  test 'a dead key with nothing to replace it still warns' do
    create(:api_key, user: @user, exchange: @binance, key_type: :trading, status: :incorrect,
                     last_sync_error: 'Invalid API-key')

    assert banner_for?('Binance')
  end

  test 'the key actually being read with is the one that speaks' do
    create(:api_key, user: @user, exchange: @binance, key_type: :read_only, status: :correct,
                     last_sync_error: 'Timed out reading the ledger')

    assert banner_for?('Binance'), 'the reading key itself failed, which is a real hole'
  end

  test 'a working venue says nothing' do
    create(:api_key, user: @user, exchange: @binance, key_type: :read_only, status: :correct)

    assert_not banner_for?('Binance')
  end
end
