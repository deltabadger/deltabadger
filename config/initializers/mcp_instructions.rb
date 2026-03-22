# frozen_string_literal: true

# Set MCP server instructions dynamically so they include the actual
# list of available exchanges from the database.
Rails.application.config.after_initialize do
  exchanges = begin
    Exchange.where(available: true).pluck(:name).join(', ')
  rescue StandardError
    'Binance, Kraken, Coinbase, Alpaca'
  end

  ActionMCP.configuration.server_instructions = [
    "Deltabadger is a user's personal investing server. Available exchanges: #{exchanges}.",
    "Supports both cryptocurrency and stocks/ETFs (via Alpaca).",
    "Trading is available either via DCA bots, or by direct access to connected exchanges",
    "When the user asks to trade stocks or ETFs (e.g., QQQM, SPY, AAPL), use the Alpaca exchange.",
    "Use list_exchanges to see which exchanges the user has connected before placing orders."
  ]
end
