require 'test_helper'

# §10 connect wizard. A dedicated 2-step Settings flow:
#   step 1: create a pending IBKR key + generate RSA/DH artifacts (bg job) -> download the 3
#           public files -> IBKR OAuth self-service portal (PUT-capable hosts only).
#   step 2: paste consumer_key / access_token / access_token_secret -> validate
#           -> :pending_activation (or :correct if IBKR already activated the consumer).
class Settings::IbkrConnectionsControllerTest < ActionDispatch::IntegrationTest
  include Devise::Test::IntegrationHelpers

  # 2048-bit RSA generation is slow to repeat; generate the fixtures once.
  SIG_KEY = OpenSSL::PKey::RSA.new(2048)
  ENC_KEY = OpenSSL::PKey::RSA.new(2048)
  DH_PARAM = <<~PEM.freeze
    -----BEGIN DH PARAMETERS-----
    MIGLAoGBAPSHspMWc9/phm3D2OBRkX50s+m2FIhbYg3DuXLrMg3lmEZlxRNVh1e2
    c3uCWZcYlMvVr1WlIjavZDukxnQJ03+l4UbiwnShAyEOtDcx+CF2AX9EW8+56seh
    kiWLnuA42ENa2+S67sxqItRI1s4IuFD6zK4+zGycXR8EABoRrZh3AgECAgIArw==
    -----END DH PARAMETERS-----
  PEM

  setup do
    create(:user, admin: true, setup_completed: true) # platform requires an admin to exist
    @user = create(:user, setup_completed: true)
    @ibkr = create(:ibkr_exchange)
    sign_in @user
  end

  # An IBKR key already through step 1 (artifacts generated, no creds yet).
  def key_with_artifacts
    @user.api_keys.create!(
      exchange: @ibkr, key_type: :trading, status: :pending_validation,
      rsa_signature_key: SIG_KEY.to_pem, rsa_encryption_key: ENC_KEY.to_pem, dh_param: DH_PARAM
    )
  end

  test 'requires authentication' do
    sign_out @user
    get settings_ibkr_connect_path
    assert_response :redirect
    assert_match(/login|sign_in/, @response.redirect_url)
  end

  test 'show renders the connect wizard' do
    get settings_ibkr_connect_path
    assert_response :success
  end

  test 'create makes a pending IBKR trading key and kicks off key generation' do
    Ibkr::GenerateConnectionKeysJob.expects(:perform_later).once

    assert_difference '@user.api_keys.count', 1 do
      post settings_ibkr_connect_path
    end
    assert @user.api_keys.find_by(exchange: @ibkr, key_type: :trading).pending_validation?
  end

  test 'create is idempotent — reuses the existing row and does not regenerate built artifacts' do
    key = key_with_artifacts
    sig = key.rsa_signature_key
    Ibkr::GenerateConnectionKeysJob.expects(:perform_later).never

    assert_no_difference '@user.api_keys.count' do
      post settings_ibkr_connect_path
    end
    assert_equal sig, key.reload.rsa_signature_key, 'existing artifacts left intact'
  end

  test 'create re-enqueues generation for an existing row that has no artifacts yet' do
    @user.api_keys.create!(exchange: @ibkr, key_type: :trading, status: :pending_validation) # blank artifacts
    Ibkr::GenerateConnectionKeysJob.expects(:perform_later).once

    assert_no_difference '@user.api_keys.count' do
      post settings_ibkr_connect_path
    end
  end

  test 'show reveals both PUT-capable portal links once keys are generated' do
    key_with_artifacts
    get settings_ibkr_connect_path

    assert_response :success
    Exchanges::Ibkr::OAUTH_PORTALS.each_value do |portal|
      assert_select 'a[href=?]', portal[:url]
    end
    assert_select 'select[data-ibkr-connect-target]', count: 0 # entity picker removed
    # The regional hosts 501 the portal's key-upload PUT — they must never reappear.
    refute_includes response.body, 'interactivebrokers.ie'
    refute_includes response.body, 'interactivebrokers.lu'
    refute_includes response.body, 'interactivebrokers.com.hu'
  end

  test 'download serves the PUBLIC signing/encryption keys and the dhparam, never the private key' do
    key_with_artifacts

    get settings_download_ibkr_connection_path(artifact: 'signing')
    assert_response :success
    assert_includes response.body, 'PUBLIC KEY'
    refute_includes response.body, 'PRIVATE KEY'

    get settings_download_ibkr_connection_path(artifact: 'encryption')
    assert_includes response.body, 'PUBLIC KEY'
    refute_includes response.body, 'PRIVATE KEY'

    get settings_download_ibkr_connection_path(artifact: 'dhparam')
    assert_includes response.body, 'DH PARAMETERS'
  end

  test 'download is scoped to the current user (no own key -> nothing served)' do
    get settings_download_ibkr_connection_path(artifact: 'signing')
    assert_redirected_to settings_ibkr_connect_path
  end

  test 'download redirects when the artifacts have not been generated yet' do
    @user.api_keys.create!(exchange: @ibkr, key_type: :trading, status: :pending_validation) # no artifacts
    get settings_download_ibkr_connection_path(artifact: 'signing')
    assert_redirected_to settings_ibkr_connect_path
  end

  test 'download redirects on an unknown artifact' do
    key_with_artifacts
    get settings_download_ibkr_connection_path(artifact: 'evil')
    assert_redirected_to settings_ibkr_connect_path
  end

  test 'activate rejects blank or omitted credentials without touching the key' do
    key = key_with_artifacts
    ApiKey.any_instance.expects(:get_validity).never

    [
      { key: '', access_token: 'TOKEN456', secret: 'SECRET789' },
      { key: 'CONSUMER12', access_token: '', secret: 'SECRET789' },
      { key: 'CONSUMER12', access_token: 'TOKEN456', secret: '' },
      { access_token: 'TOKEN456', secret: 'SECRET789' } # key omitted entirely
    ].each do |params|
      post settings_activate_ibkr_connection_path, params: { api_key: params }

      assert_redirected_to settings_ibkr_connect_path
      assert_equal I18n.t('settings.ibkr.missing_fields'), flash[:alert]
      key.reload
      assert key.pending_validation?, "status must not change for #{params.inspect}"
      assert_nil key.key, 'credentials must not be assigned'
      assert_equal SIG_KEY.to_pem, key.rsa_signature_key, 'generated artifacts untouched'
    end
  end

  test 'blank resubmit from pending_activation keeps the key pending_activation' do
    key = key_with_artifacts
    key.update!(status: :pending_activation)
    ApiKey.any_instance.expects(:get_validity).never

    post settings_activate_ibkr_connection_path,
         params: { api_key: { key: '', access_token: 'T', secret: 'S' } }

    assert_equal I18n.t('settings.ibkr.missing_fields'), flash[:alert]
    assert key.reload.pending_activation?, 'escape-hatch blank submit must not change the state'
  end

  test 'pending_activation state offers the credentials form so mistakes are correctable' do
    key_with_artifacts.update!(status: :pending_activation)
    get settings_ibkr_connect_path

    assert_response :success
    assert_select 'form[action=?]', settings_activate_ibkr_connection_path
    assert_select "input[name='api_key[key]']"
    assert_includes response.body, I18n.t('settings.ibkr.reenter_intro')
    assert_includes response.body, I18n.t('settings.ibkr.start_over')
  end

  test 'activate marks the key pending_activation and preserves the generated keys' do
    key = key_with_artifacts
    # Bypass the exchange/Dryable boundary: drive the status from the validity result directly.
    ApiKey.any_instance.stubs(:get_validity).returns(Result::Success.new(:pending_activation))

    post settings_activate_ibkr_connection_path,
         params: { api_key: { key: 'CONSUMER123', access_token: 'TOKEN456', secret: 'SECRET789' } }

    key.reload
    assert key.pending_activation?
    assert_equal 'CONSUMER123', key.key
    assert_equal 'TOKEN456', key.access_token
    assert_equal SIG_KEY.to_pem, key.rsa_signature_key, 'generated signing key preserved (assign_credentials gotcha)'
    assert_equal ENC_KEY.to_pem, key.rsa_encryption_key, 'generated encryption key preserved'
    assert_equal DH_PARAM, key.dh_param, 'generated DH params preserved'
  end

  test 'activate marks the key correct when IBKR already reports accounts' do
    key_with_artifacts
    ApiKey.any_instance.stubs(:get_validity).returns(Result::Success.new(true))

    post settings_activate_ibkr_connection_path,
         params: { api_key: { key: 'C', access_token: 'T', secret: 'S' } }

    assert @user.api_keys.find_by(exchange: @ibkr, key_type: :trading).correct?
  end

  test 'activate redirects when there is no connection in progress' do
    post settings_activate_ibkr_connection_path,
         params: { api_key: { key: 'C', access_token: 'T', secret: 'S' } }
    assert_redirected_to settings_ibkr_connect_path
  end

  test 'destroy stops dependent bots and removes the in-progress connection' do
    key_with_artifacts
    # Don't leave bots firing against a deleted credential (mirrors the settings key-deletion flow).
    ApiKey.any_instance.expects(:stop_dependent_bots!).once

    assert_difference '@user.api_keys.count', -1 do
      delete settings_ibkr_connect_path
    end
    assert_redirected_to settings_ibkr_connect_path
  end
end
