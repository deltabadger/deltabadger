require 'test_helper'

# Characterization tests for the index confirm-settings defaults, written before
# single-sourcing the wizard default settings: #new must apply the same defaults
# as Bots::DcaIndexes::PickSpendableAssetsController#finalise_and_redirect
# (quote_amount 100, weekly interval, allocation_flattening 0.0 — num_coins is
# owned by the model). The controller itself stays custom (out of refactor scope).
class Bots::DcaIndexes::ConfirmSettingsControllerTest < ActionDispatch::IntegrationTest
  setup do
    @user = create(:user, admin: true, setup_completed: true)
    sign_in @user
    MarketDataSettings.stubs(:current_provider).returns(MarketDataSettings::PROVIDER_DELTABADGER)
    @kraken = create(:kraken_exchange)
  end

  def seed_wizard_session
    post bots_dca_indexes_pick_index_path, params: { index_type: Bots::DcaIndex::INDEX_TYPE_TOP }
    post bots_dca_indexes_pick_exchange_path,
         params: { bots_dca_index: { exchange_id: @kraken.id } }
  end

  test 'new applies the wizard default settings to the session' do
    seed_wizard_session
    get new_bots_dca_indexes_confirm_settings_path
    assert_response :success

    settings = session[:bot_config]['settings']
    assert_equal 100, settings['quote_amount']
    assert_equal 'week', settings['interval']
    assert_equal 0.0, settings['allocation_flattening']
    assert_nil settings['num_coins'], 'num_coins default is owned by the model'
  end

  test 'new redirects to the index step when the wizard session lacks an exchange' do
    get new_bots_dca_indexes_confirm_settings_path
    assert_redirected_to new_bots_dca_indexes_pick_index_path
  end
end
