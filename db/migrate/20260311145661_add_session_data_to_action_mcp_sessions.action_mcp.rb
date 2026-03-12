# frozen_string_literal: true

# This migration comes from action_mcp (originally 20260303000001)
class AddSessionDataToActionMCPSessions < ActiveRecord::Migration[8.1]
  def change
    return if column_exists?(:action_mcp_sessions, :session_data)

    add_column :action_mcp_sessions, :session_data, :json, default: {}, null: false
  end
end
