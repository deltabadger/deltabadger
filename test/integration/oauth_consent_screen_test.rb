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

  # The personal API application must never be usable at /oauth/authorize: ToolAccess
  # reads a token on it as the user acting as themselves, and it would appear in no
  # connected-clients list.
  test 'the personal api application cannot be driven through the oauth flow' do
    personal = @user.personal_api_token.application

    get '/oauth/authorize', params: {
      client_id: personal.uid, redirect_uri: 'https://localhost/personal-access-token',
      response_type: 'code', scope: 'api',
      code_challenge: 'E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM',
      code_challenge_method: 'S256'
    }

    assert_not_equal 200, response.status, 'the consent screen must not render'
    assert_no_match(/#{Regexp.escape(I18n.t('settings.mcp.authorize_title'))}/, response.body)
  end

  test 'the unused client-management pages are not routed' do
    # recognize_path raises directly rather than going through a full request, which
    # matters here: this suite's test environment renders routing errors as a debug
    # page instead of letting them propagate through `get` (see
    # bots/exchange_first_creation_test.rb for the same pattern already in use).
    routes = Rails.application.routes
    assert_raises(ActionController::RoutingError) { routes.recognize_path('/oauth/applications') }
    assert_raises(ActionController::RoutingError) { routes.recognize_path('/oauth/authorized_applications') }
  end

  test 'the authorization endpoints still route' do
    assert_recognizes({ controller: 'doorkeeper/authorizations', action: 'new' }, '/oauth/authorize')
    assert_recognizes({ controller: 'doorkeeper/tokens', action: 'create' },
                      path: '/oauth/token', method: :post)
  end

  # form_post renders a page whose script submits a form to the client's
  # redirect_uri, which form-action 'self' does not permit. The metadata this app
  # publishes advertises query and fragment only, so refusing it is what the
  # metadata already says.
  test 'form_post is refused rather than rendered' do
    get '/oauth/authorize', params: {
      client_id: @application.uid,
      redirect_uri: @application.redirect_uri,
      response_type: 'code',
      response_mode: 'form_post',
      code_challenge: 'a' * 43,
      code_challenge_method: 'S256',
      scope: 'mcp',
      state: 'xyz'
    }

    # handle_auth_errors defaults to :render, so this renders rather than redirects.
    assert_response :bad_request
    assert_select 'form[name=redirect_form]', false, 'the auto-submitting page must not render'
    assert_includes response.body, 'does not support this response mode'
  end

  # Companion to the refusal test above: proves the same parameter set reaches the
  # consent screen when response_mode is absent, so the refusal above is actually
  # about response_mode and not some other malformed parameter.
  test 'the same request without a response mode reaches consent' do
    get '/oauth/authorize', params: {
      client_id: @application.uid,
      redirect_uri: @application.redirect_uri,
      response_type: 'code',
      code_challenge: 'a' * 43,
      code_challenge_method: 'S256',
      scope: 'mcp',
      state: 'xyz'
    }

    assert_response :success
  end
end
