# frozen_string_literal: true

class ListExchangesTool < ApplicationMCPTool
  tool_name 'list_exchanges'
  description 'List connected exchanges with API key status'
  read_only

  def perform
    result = BotApi::Exchanges::List.call(user: current_user)
    data = result.data

    if data[:count].zero?
      render text: 'No exchanges connected. Add an API key when creating a bot.'
      return
    end

    lines = data[:exchanges].map { |ex| "- #{ex[:name]} | API key status: #{ex[:api_key_status]}" }
    render text: "Connected Exchanges (#{data[:count]}):\n#{lines.join("\n")}"
  end
end
