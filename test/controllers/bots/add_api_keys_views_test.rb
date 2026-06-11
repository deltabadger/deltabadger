require 'test_helper'

# Characterization tests for the add-API-keys wizard step views, written before
# extracting the shared bots/add_api_keys partials: `new` renders the API key form
# in the full-page modal; a failed `create` re-renders the form into the modal via
# turbo stream. The wizard session is seeded through real requests (same pattern as
# Bots::DcaSingleAssets::PickExchangesControllerTest).
module AddApiKeysViewBehaviorTests
  extend ActiveSupport::Concern

  TURBO_STREAM_ACCEPT = 'text/vnd.turbo-stream.html, text/html'.freeze

  included do
    setup do
      @user = create(:user, admin: true, setup_completed: true)
      sign_in @user
      seed_wizard_session
      # In tests api keys count as correct (dry run), which would skip the form
      # entirely — force the real-user path where a key still has to be entered.
      ApiKey.any_instance.stubs(:correct?).returns(false)
    end

    test 'new renders the api key form in the full-page modal' do
      get add_api_key_path
      assert_response :success
      assert_select 'form'
    end

    test 'failed create re-renders the form into the modal via turbo stream' do
      ApiKey.any_instance.stubs(:validate_credentials!)
      ApiKey.any_instance.stubs(:incorrect?).returns(true)

      post add_api_keys_path,
           params: { api_key: { key: 'k', secret: 's' } },
           headers: { 'Accept' => TURBO_STREAM_ACCEPT }

      assert_response :unprocessable_entity
      assert_match 'turbo-stream', response.body
      assert_match 'modal_content', response.body
    end
  end
end

class Bots::DcaSingleAssets::AddApiKeysViewsTest < ActionDispatch::IntegrationTest
  include AddApiKeysViewBehaviorTests

  private

  def add_api_key_path = new_bots_dca_single_assets_add_api_key_path
  def add_api_keys_path = bots_dca_single_assets_add_api_key_path

  def seed_wizard_session
    btc = create(:asset, :bitcoin)
    usd = create(:asset, :usd)
    binance = create(:binance_exchange)
    create(:ticker, :btc_usd, exchange: binance, base_asset: btc, quote_asset: usd)

    get new_bots_dca_single_assets_pick_buyable_asset_path
    post bots_dca_single_assets_pick_buyable_asset_path,
         params: { bots_dca_single_asset: { base_asset_id: btc.id } }
    post bots_dca_single_assets_pick_exchange_path,
         params: { bots_dca_single_asset: { exchange_id: binance.id } }
  end
end

class Bots::DcaDualAssets::AddApiKeysViewsTest < ActionDispatch::IntegrationTest
  include AddApiKeysViewBehaviorTests

  private

  def add_api_key_path = new_bots_dca_dual_assets_add_api_key_path
  def add_api_keys_path = bots_dca_dual_assets_add_api_key_path

  def seed_wizard_session
    btc = create(:asset, :bitcoin)
    eth = create(:asset, :ethereum)
    usd = create(:asset, :usd)
    binance = create(:binance_exchange)
    create(:ticker, :btc_usd, exchange: binance, base_asset: btc, quote_asset: usd)
    create(:ticker, :eth_usd, exchange: binance, base_asset: eth, quote_asset: usd)

    get new_bots_dca_single_assets_pick_buyable_asset_path
    post bots_dca_single_assets_pick_buyable_asset_path,
         params: { bots_dca_single_asset: { base_asset_id: btc.id } }
    post promote_to_dual_bots_dca_single_assets_pick_exchange_path
    post bots_dca_dual_assets_pick_second_buyable_asset_path,
         params: { bots_dca_dual_asset: { base1_asset_id: eth.id } }
    post bots_dca_dual_assets_pick_exchange_path,
         params: { bots_dca_dual_asset: { exchange_id: binance.id } }
  end
end
