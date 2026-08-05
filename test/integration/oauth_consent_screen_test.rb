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

  def authorize!(redirect_uri: 'https://evil.test/cb')
    get '/oauth/authorize', params: {
      client_id: @application.uid, redirect_uri: redirect_uri,
      response_type: 'code', scope: 'mcp api',
      code_challenge: 'E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM',
      code_challenge_method: 'S256'
    }
  end

  test 'shows the redirect host so the owner can see where the code goes' do
    authorize!
    assert_response :success
    # The hidden form fields also carry the full redirect_uri as an attribute value, so a
    # plain substring match on 'evil.test' would pass even without a visible label — assert
    # on the label and the <code> element specifically, which only the visible copy renders.
    assert_includes response.body, I18n.t('settings.mcp.authorize_redirect_label')
    assert_includes response.body, '<code>evil.test</code>'
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
    # Positive + negative in one assertion: refute_includes alone would pass vacuously
    # if the page ever 500s or redirects instead of rendering the name at all.
    assert_includes response.body, '&lt;img src=x onerror=alert(1)&gt;'
    refute_includes response.body, '<img src=x onerror=alert(1)>'
  end

  test 'falls back to the raw redirect_uri when a legacy registration has no host' do
    # Pre-dates the http(s) allowlist in the registration controller: Doorkeeper's own
    # validator accepts native/custom schemes like this one (see
    # dynamic_registration_controller_test.rb), so applications registered before this
    # change can still have a redirect_uri with no host component.
    @application.update!(redirect_uri: 'myapp:/cb')

    authorize!(redirect_uri: 'myapp:/cb')

    assert_response :success
    assert_includes response.body, '<code>myapp:/cb</code>'
  end
end
