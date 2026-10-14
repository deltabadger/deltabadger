# A client connected from now on stores no tool list (NULL) and follows the user's own
# Tool permissions. A stored list stays what it was.
#
# A client that holds a live credential but has no connected_clients row can use nothing
# today. It gets an explicit empty list here, so that from now on a missing row at consent
# can only mean a new connection.
class LetConnectedClientsFollowTheUser < ActiveRecord::Migration[8.1]
  def up
    change_column_null :connected_clients, :mcp_tools, true
    change_column_null :connected_clients, :rest_tools, true

    now = connection.quote(Time.current.utc)
    execute <<~SQL.squish
      INSERT INTO connected_clients (user_id, oauth_application_id, mcp_tools, rest_tools, created_at, updated_at)
      SELECT DISTINCT live.resource_owner_id, live.application_id, '[]', '[]', #{now}, #{now}
      FROM (
        SELECT resource_owner_id, application_id FROM oauth_access_tokens
        WHERE revoked_at IS NULL AND resource_owner_id IS NOT NULL
        UNION
        SELECT resource_owner_id, application_id FROM oauth_access_grants
        WHERE revoked_at IS NULL AND datetime(created_at, '+' || expires_in || ' seconds') > #{now}
      ) AS live
      JOIN users ON users.id = live.resource_owner_id
      JOIN oauth_applications ON oauth_applications.id = live.application_id
        AND oauth_applications.personal_access_token = 0
      WHERE NOT EXISTS (
        SELECT 1 FROM connected_clients
        WHERE connected_clients.user_id = live.resource_owner_id
          AND connected_clients.oauth_application_id = live.application_id
      )
    SQL
  end

  # A following client drops to an empty list: it can use nothing until the user grants
  # tools in Settings. The value is the stored JSON text, because the adapter cannot quote
  # a Ruby Array here.
  def down
    change_column_null :connected_clients, :mcp_tools, false, '[]'
    change_column_null :connected_clients, :rest_tools, false, '[]'
  end
end
