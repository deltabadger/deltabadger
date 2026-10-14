# frozen_string_literal: true

module ConnectedClients
  # Records an approved authorization. The consent screen asks nothing about tools, so
  # nothing from the request is read.
  #
  # A new connection gets a row with no tool lists (NULL), so the client follows the
  # user's own Tool permissions. An existing row is left exactly as it is. Re-approving
  # never widens or narrows what the user set for this client in Settings.
  #
  # "No row" can only mean a new connection: every client that held a live credential
  # without a row was given an explicit empty one by the migration that introduced NULL.
  class RecordConsent
    def self.call(grant:)
      new(grant: grant).call
    end

    def initialize(grant:)
      @grant = grant
    end

    def call
      # The same Doorkeeper hook also fires from the token endpoint, where there is
      # no grant to read.
      return nil unless @grant.is_a?(Doorkeeper::AccessGrant)
      return nil unless User.exists?(id: @grant.resource_owner_id)

      # The block runs only on insert. Two tabs approving at once both end up with
      # the one row, never with an error after the code is already committed.
      ConnectedClient.create_or_find_by!(user_id: @grant.resource_owner_id, oauth_application_id: @grant.application_id) do |client|
        client.mcp_tools = nil
        client.rest_tools = nil
      end
    end
  end
end
