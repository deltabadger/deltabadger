class Exchange::FetchPricesJob < ApplicationJob
  queue_as :default

  def perform(exchange)
    result = exchange.get_tickers_prices(force: true)
    raise StandardError, result.errors.to_sentence unless result.success?
  end
end
