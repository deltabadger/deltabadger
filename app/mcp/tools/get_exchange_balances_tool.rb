# frozen_string_literal: true

class GetExchangeBalancesTool < ApplicationMCPTool
  tool_name 'get_exchange_balances'
  description "Fetch live balances from a connected exchange. Requires the exchange name (e.g., 'Binance', 'Kraken', 'Coinbase')."
  read_only
  open_world

  property :exchange_name, type: 'string', required: true, description: 'Exchange name (e.g., Binance, Kraken, Coinbase)'

  def perform
    user = current_user
    exchange = Exchange.where('LOWER(name) = ?', exchange_name.downcase).first

    unless exchange
      render text: "Exchange '#{exchange_name}' not found. Available exchanges: #{Exchange.where(available: true).pluck(:name).join(', ')}"
      return
    end

    api_key = user.api_keys.find_by(exchange: exchange, key_type: :trading, status: :correct)
    unless api_key
      render text: "No valid API key found for #{exchange.name}. Please add an API key in Settings."
      return
    end

    exchange.set_client(api_key: api_key)
    result = exchange.get_balances

    unless result.success?
      render text: "Failed to fetch balances from #{exchange.name}: #{result.errors.join(', ')}"
      return
    end

    balances = result.data
    if balances.blank?
      render text: "No balances found on #{exchange.name}."
      return
    end

    lines = balances.filter_map do |asset_id, balance|
      free = balance[:free].to_f
      locked = balance[:locked].to_f
      next if free.zero? && locked.zero?

      asset = Asset.find_by(id: asset_id)
      symbol = asset&.symbol || "Unknown(#{asset_id})"

      locked_str = locked.positive? ? " (#{locked} locked)" : ''
      "- #{symbol}: #{free}#{locked_str}"
    end

    if lines.empty?
      render text: "All balances on #{exchange.name} are zero."
      return
    end

    render text: "#{exchange.name} Balances:\n#{lines.join("\n")}"
  end
end
