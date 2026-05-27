# frozen_string_literal: true

class GetExchangeBalancesTool < ApplicationMCPTool
  tool_name 'get_exchange_balances'
  description "Fetch live balances from a connected exchange. Requires the exchange name (e.g., 'Binance', 'Kraken', 'Coinbase')."
  read_only
  open_world

  property :exchange_name, type: 'string', required: true, description: 'Exchange name (e.g., Binance, Kraken, Coinbase)'

  def perform
    result = BotApi::Exchanges::Balances.call(user: current_user, exchange_name: exchange_name)
    return render(text: result.error_message) unless result.success?

    data = result.data
    if data[:count].zero?
      render text: "All balances on #{data[:exchange]} are zero."
      return
    end

    lines = data[:balances].map do |row|
      locked_str = row[:locked].positive? ? " (#{row[:locked]} locked)" : ''
      "- #{row[:symbol]}: #{row[:free]}#{locked_str}"
    end
    render text: "#{data[:exchange]} Balances:\n#{lines.join("\n")}"
  end
end
