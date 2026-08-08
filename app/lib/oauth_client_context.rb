# frozen_string_literal: true

# The OAuth client behind the current MCP request.
#
# MCP has no other way to carry it. ActionMCP assigns exactly one identity per
# request — Gateway#authenticate! returns from inside its loop — and rejects any
# identity key outside its own whitelist, so a second `identified_by` is not an
# option; ActionMCP::Current has no attribute for it; and Gateway keeps its Rack
# request in an ivar with no reader, so a tool cannot re-read the bearer header for
# itself.
#
# Set once per request from MCPTokenIdentifier#resolve, read by ApplicationGateway
# and ApplicationMCPTool. Cleared by the Rails executor at the request boundary,
# like any ActiveSupport::CurrentAttributes, so it cannot leak across requests on a
# reused Puma thread.
#
# Not to be confused with ActionMCP::Current, which is the gem's own.
class OauthClientContext < ActiveSupport::CurrentAttributes
  attribute :oauth_application
end
