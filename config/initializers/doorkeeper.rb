# frozen_string_literal: true

# The authorization-server metadata this app publishes lists no
# response_modes_supported, so RFC 8414's default is what it advertises: query and
# fragment. form_post is neither, and its response posts a form to the client's
# redirect_uri — which form-action 'self' does not permit, so it could not complete
# even if the page rendered. Narrowing the flow lets Doorkeeper refuse it through
# its own validation, so the condition is reported with the library's own error and
# localized message rather than one hand-rolled here, and there is no controller
# code to keep in step with its validation order.
#
# The delete comes first because the registry warns when it overrides a registered
# flow, and this runs on every boot. `flows` is a public accessor. Everything else
# mirrors the gem's own registration; `query` stays first, so the default response
# mode is unchanged.
Doorkeeper::GrantFlow.flows.delete(:authorization_code)
Doorkeeper::GrantFlow.register(
  :authorization_code,
  response_type_matches: 'code',
  response_type_strategy: Doorkeeper::Request::Code,
  grant_type_matches: 'authorization_code',
  grant_type_strategy: Doorkeeper::Request::AuthorizationCode,
  response_mode_matches: %w[query fragment]
)

Doorkeeper.configure do
  orm :active_record

  # Devise integration
  resource_owner_authenticator do
    current_user || warden.authenticate!(scope: :user)
  end

  # OAuth 2.1: authorization_code only
  grant_flows %w[authorization_code]

  # PKCE required for all clients (OAuth 2.1)
  force_ssl_in_redirect_uri false # allow http for localhost dev
  force_pkce

  access_token_expires_in 1.hour
  use_refresh_token

  default_scopes :mcp
  optional_scopes :api

  # Always show consent screen — any client can self-register via DCR
  skip_authorization do
    false
  end

  # The personal API token's application exists only so User#mint_personal_token!
  # has something to hang a token on. It is a public client with a real redirect URI,
  # and Doorkeeper does not check an application's grant_types against the requested
  # response type, so nothing else keeps it out of the authorization flow.
  #
  # It has to stay out. ToolAccess reads a token on a personal application as the user
  # acting as themselves, which is only ever true of the one mint_personal_token!
  # issues, and such a token would appear in no connected-clients list. This
  # application has no legitimate OAuth flow: refuse it every one.
  allow_grant_flow_for_client do |_grant_flow, client|
    !client.respond_to?(:personal_access_token?) || !client.personal_access_token?
  end

  # Custom base controller to avoid ApplicationController filters
  base_controller 'Oauth::BaseController'

  allow_blank_redirect_uri false

  # Fires after the authorization code has been created. Gated to POST because the
  # same hook also runs on the GET auto-approve path, where the parameters are the
  # *client's* query string rather than our consent form — a client could otherwise
  # name its own permission groups in the authorize URL. The token endpoint calls
  # this hook too, with a response that has no `auth`, so RecordConsent no-ops there.
  after_successful_authorization do |controller, context|
    if controller.request.post?
      ConnectedClients::RecordConsent.call(
        grant: context.auth.try(:auth).try(:token),
        mcp_groups: controller.params[:granted_mcp_groups],
        rest_groups: controller.params[:granted_rest_groups]
      )
    end
  end
end
