# frozen_string_literal: true

class ListIndicesTool < ApplicationMCPTool
  tool_name 'list_indices'
  description 'List the indices an index bot can track (Top coins, CoinGecko categories, stock indices) ' \
              'and the exchanges each is available on'
  read_only

  property :exchange_name, type: 'string', description: 'Only indices available on this exchange (optional)'

  def perform
    result = BotApi::Indices::List.call(exchange_name: exchange_name)
    return render(text: result.error_message) unless result.success?

    lines = result.data[:indices].map do |row|
      "- #{row[:id]} | #{row[:name]} | #{row[:coins]} assets | #{row[:exchanges].join(', ').presence || 'no exchange'}"
    end
    render text: "Indices (#{result.data[:count]}):\n#{lines.join("\n")}"
  end
end
