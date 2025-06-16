module Bot::OrderCreator
  extend ActiveSupport::Concern

  def create_submitted_order!(order_data)
    order_values = base_order_values.merge(
      status: :submitted,
      external_status: order_data[:status],
      external_id: order_data[:order_id],
      price: order_data[:price],
      amount: order_data[:amount],
      quote_amount: order_data[:quote_amount],
      base: order_data[:ticker].base_asset.symbol,
      quote: order_data[:ticker].quote_asset.symbol,
      side: order_data[:side],
      order_type: order_data[:order_type],
      amount_exec: order_data[:amount_exec],
      quote_amount_exec: order_data[:quote_amount_exec]
    ).compact
    transactions.create!(order_values)
  end

  def create_failed_order!(order_data)
    order_values = base_order_values.merge(
      status: :failed,
      external_status: order_data[:status],
      external_id: order_data[:order_id],
      error_messages: order_data[:error_messages],
      price: order_data[:price],
      amount: order_data[:amount],
      quote_amount: order_data[:quote_amount],
      base: order_data[:ticker].base_asset.symbol,
      quote: order_data[:ticker].quote_asset.symbol,
      side: order_data[:side],
      order_type: order_data[:order_type],
      amount_exec: 0,
      quote_amount_exec: 0
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
      quote: order_data[:ticker].quote_asset.symbol,
      side: order_data[:side],
      order_type: order_data[:order_type],
      amount_exec: 0,
      quote_amount_exec: 0
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
