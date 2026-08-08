require 'test_helper'

# The only test that drives a real MCP request end to end. Everything else asserts
# the pieces; this proves the chain — identifier resolves the client, gateway writes
# the intersection into the session registry, dispatch honours it.
class McpRequestPermissionsTest < ActionDispatch::IntegrationTest
  setup do
    # config/mcp.yml pins authentication: ["none"] for the test environment, which
    # makes every MCP request 401. Stub it here rather than changing the shared file.
    ActionMCP.configuration.stubs(:authentication_methods).returns(['bearer_token'])

    @user = create(:user, admin: true)
    @application = Doorkeeper::Application.create!(
      name: 'Test client', redirect_uri: 'http://localhost/callback',
      confidential: false, scopes: 'mcp'
    )
    @token = Doorkeeper::AccessToken.create!(
      application: @application, resource_owner_id: @user.id,
      token: SecureRandom.hex(32), scopes: 'mcp', expires_in: 3600
    )
    ConnectedClient.create!(user: @user, oauth_application: @application, mcp_tools: %w[list_bots])
  end

  def rpc(body, session_id: nil)
    headers = {
      'Authorization' => "Bearer #{@token.token}",
      'Content-Type' => 'application/json',
      'Accept' => 'application/json, text/event-stream'
    }
    headers['Mcp-Session-Id'] = session_id if session_id
    post '/mcp', params: body.to_json, headers: headers
  end

  # The handshake is two steps. Without the initialized notification the session
  # stays "initializing" and every later call is refused with "Session
  # initialization is incomplete." — which would make the refusal assertions below
  # pass for entirely the wrong reason.
  def initialize_session
    rpc({ jsonrpc: '2.0', id: 1, method: 'initialize',
          params: { protocolVersion: '2025-06-18', capabilities: {},
                    clientInfo: { name: 'test', version: '1' } } })
    session_id = response.headers['Mcp-Session-Id']
    assert session_id.present?, "no session id in #{response.status}: #{response.body}"

    rpc({ jsonrpc: '2.0', method: 'notifications/initialized' }, session_id: session_id)
    assert_response :accepted

    session_id
  end

  test 'tools/list shows only what this client was granted' do
    session_id = initialize_session

    rpc({ jsonrpc: '2.0', id: 2, method: 'tools/list', params: {} }, session_id: session_id)

    names = response.parsed_body.dig('result', 'tools').to_a.map { |t| t['name'] }
    assert_equal %w[list_bots], names
  end

  test 'a granted tool runs end to end' do
    session_id = initialize_session

    rpc({ jsonrpc: '2.0', id: 3, method: 'tools/call',
          params: { name: 'list_bots', arguments: {} } }, session_id: session_id)

    body = response.parsed_body
    assert_nil body['error'], body.inspect
    assert_not_equal true, body.dig('result', 'isError'), body.inspect
  end

  test 'the registry refuses a tool the user has on but this client lacks' do
    session_id = initialize_session

    assert @user.mcp_tool_enabled?('list_exchanges')
    rpc({ jsonrpc: '2.0', id: 4, method: 'tools/call',
          params: { name: 'list_exchanges', arguments: {} } }, session_id: session_id)

    # Not registered for this session, so the gem's own dispatch rejects it with
    # invalid_params before ApplicationMCPTool#call is ever reached.
    assert_equal(-32_602, response.parsed_body.dig('error', 'code'), response.parsed_body.inspect)
  end

  # There is deliberately no HTTP test that reaches the per-call gate on a REFUSAL.
  # configure_session recomputes the registry from the same ToolAccess on every
  # request, so over the wire the registry always refuses first — the two layers
  # cannot disagree within one request. That is the point of the second layer: it
  # only matters if the registry is ever stale. The success case above does pass
  # through it, and its refusal branches are covered directly in
  # test/mcp/per_client_tool_permissions_test.rb.

  test 'an unauthenticated request never reaches a tool' do
    post '/mcp', params: { jsonrpc: '2.0', id: 1, method: 'tools/list', params: {} }.to_json,
                 headers: { 'Content-Type' => 'application/json',
                            'Accept' => 'application/json, text/event-stream' }

    assert_includes [401, 403], response.status
  end
end
