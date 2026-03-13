# frozen_string_literal: true

class ApplicationGateway < ActionMCP::Gateway
  identified_by MCPTokenIdentifier

  def configure_session(session)
    session.session_data = { 'user_id' => user.id }
  end
end
