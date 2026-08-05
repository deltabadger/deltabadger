require 'test_helper'

# The owner must be able to see WHAT is being granted and WHERE the code goes —
# both were previously hidden form fields only.
class OauthConsentScreenTest < ActionDispatch::IntegrationTest
  include Devise::Test::IntegrationHelpers

  setup do
    create(:user, admin: true, setup_completed: true)
    @user = create(:user, setup_completed: true)
    sign_in @user

    # Named @application, not @app: ActionDispatch::Integration::Runner uses the
    # instance variable @app internally for the Rack app under test, so assigning
    # @app here would clobber it and break every request made in this test.
    @application = Doorkeeper::Application.create!(
      name: 'Totally Legit', redirect_uri: 'https://evil.test/cb',
      confidential: false, scopes: 'mcp api',
      token_endpoint_auth_method: 'none', grant_types: 'authorization_code',
      response_types: 'code'
    )
  end

  def authorize!
    get '/oauth/authorize', params: {
      client_id: @application.uid, redirect_uri: 'https://evil.test/cb',
      response_type: 'code', scope: 'mcp api',
      code_challenge: 'E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM',
      code_challenge_method: 'S256'
    }
  end

  test 'shows the redirect host so the owner can see where the code goes' do
    authorize!
    assert_response :success
    assert_includes response.body, 'evil.test'
  end

  test 'shows the requested scopes in plain language' do
    authorize!
    assert_includes response.body, I18n.t('settings.mcp.scope_mcp')
    assert_includes response.body, I18n.t('settings.mcp.scope_api')
  end

  test 'warns that a self-registered client is unverified' do
    authorize!
    assert_includes response.body, I18n.t('settings.mcp.authorize_unverified')
  end

  test 'escapes a client name containing markup' do
    @application.update!(name: '<img src=x onerror=alert(1)>')
    authorize!
    refute_includes response.body, '<img src=x onerror=alert(1)>'
  end
end
