module Bot::OrderCreator
  extend ActiveSupport::Concern

  def create_successful_order!(order_data)
    order_values = base_order_values.merge(
      status: :success,
      external_id: order_data[:order_id],
      price: order_data[:price],
      amount: order_data[:amount],
      quote_amount: order_data[:quote_amount],
      base: order_data[:ticker].base_asset.symbol,
      quote: order_data[:ticker].quote_asset.symbol
    ).compact
    transactions.create!(order_values)
  end

  def create_failed_order!(order_data)
    order_values = base_order_values.merge(
      status: :failure,
      external_id: order_data[:order_id],
      errors: order_data[:errors],
      price: order_data[:price],
      amount: order_data[:amount],
      quote_amount: order_data[:quote_amount],
      base: order_data[:ticker].base_asset.symbol,
      quote: order_data[:ticker].quote_asset.symbol
    ).compact
    transactions.create!(order_values)
  end

  private

  def create_skipped_order!(order_data)
    order_values = base_order_values.merge(
      status: :skipped,
      price: order_data[:price],
      amount: order_data[:amount],
      quote_amount: order_data[:quote_amount],
      base: order_data[:ticker].base_asset.symbol,
      quote: order_data[:ticker].quote_asset.symbol
    ).compact
    transactions.create!(order_values)
  end

  def base_order_values
    {
      bot_interval: interval,
      bot_quote_amount: quote_amount,
      transaction_type: 'REGULAR',
      exchange: exchange
    }
  end
end
