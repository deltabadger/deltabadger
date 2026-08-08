# frozen_string_literal: true

module ConnectedClients
  # Turns an approved authorization into the client's tool grant.
  #
  # What gets stored is the groups the user left checked on the consent screen,
  # intersected with what the user actually has enabled. Storing the intersection
  # rather than the group membership is the whole point: the grant is a snapshot,
  # so a tool enabled months later is not retroactively handed to a client that was
  # connected before it existed.
  class RecordConsent
    SURFACES = {
      mcp: { scope: 'mcp', groups: AppConfig::MCP_TOOL_GROUPS },
      rest: { scope: 'api', groups: AppConfig::REST_TOOL_GROUPS }
    }.freeze

    def self.call(grant:, mcp_groups:, rest_groups:)
      new(grant: grant, mcp_groups: mcp_groups, rest_groups: rest_groups).call
    end

    def initialize(grant:, mcp_groups:, rest_groups:)
      @grant = grant
      @mcp_groups = Array(mcp_groups).map(&:to_s)
      @rest_groups = Array(rest_groups).map(&:to_s)
    end

    def call
      # The same Doorkeeper hook also fires from the token endpoint, where there is
      # no grant to read.
      return nil unless @grant.is_a?(Doorkeeper::AccessGrant)

      user = User.find_by(id: @grant.resource_owner_id)
      return nil unless user

      write(user)
    end

    private

    # The unique index is on (user_id, oauth_application_id). Two tabs approving at
    # once would otherwise raise after the authorization code is already committed,
    # leaving the user with a 500 and no redirect. Re-read and retry once.
    def write(user, retried: false)
      client = ConnectedClient.find_or_initialize_by(
        user_id: user.id, oauth_application_id: @grant.application_id
      )
      client.mcp_tools = tools_for(user, :mcp, client)
      client.rest_tools = tools_for(user, :rest, client)
      client.save!
      client
    rescue ActiveRecord::RecordNotUnique
      raise if retried

      write(user, retried: true)
    end

    # A grant that does not carry a surface's scope says nothing about that surface,
    # so it must leave it alone. Zeroing it instead would let an mcp-only
    # re-authorization silently strip a REST grant the user has no way to re-create —
    # nothing in the UI can start an api-scoped authorization.
    def tools_for(user, surface, client)
      config = SURFACES.fetch(surface)
      existing = surface == :mcp ? client.granted_mcp_tools : client.granted_rest_tools
      return existing unless @grant.scopes.exists?(config[:scope])

      checked = surface == :mcp ? @mcp_groups : @rest_groups
      requested = config[:groups].values_at(*checked).compact.flatten
      enabled = surface == :mcp ? user.enabled_mcp_tool_names : user.enabled_rest_tool_names

      enabled & requested
    end
  end
end
