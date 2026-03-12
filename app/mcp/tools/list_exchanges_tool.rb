# frozen_string_literal: true

class ListExchangesTool < ApplicationMCPTool
  tool_name 'list_exchanges'
  description 'List connected exchanges with API key status'
  read_only

  def perform
    user = current_user
    api_keys = user.api_keys.includes(:exchange).where(key_type: :trading)

    if api_keys.empty?
      render text: 'No exchanges connected. Add an API key when creating a bot.'
      return
    end

    lines = api_keys.map do |key|
      "- #{key.exchange.name} | API key status: #{key.status}"
    end

    render text: "Connected Exchanges (#{api_keys.size}):\n#{lines.join("\n")}"
  end
end
