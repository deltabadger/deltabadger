# frozen_string_literal: true

module BotApi
  module Exchanges
    # Lists exchanges the user has *trading* API keys for. Stays scoped to
    # trading keys (not withdrawal-only) because that's what the MCP tool
    # exposes — switching to all key types would surface exchanges the user
    # can't actually trade on.
    class List
      def self.call(user:)
        new(user: user).call
      end

      def initialize(user:)
        @user = user
      end

      def call
        api_keys = @user.api_keys.includes(:exchange).where(key_type: :trading)
        rows = api_keys.map do |key|
          {
            id: key.exchange.id,
            name: key.exchange.name,
            api_key_status: key.status.to_s
          }
        end

        Result.success({ count: rows.size, exchanges: rows })
      end
    end
  end
end
