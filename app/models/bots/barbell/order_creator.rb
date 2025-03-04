module Bots::Barbell::OrderCreator
  extend ActiveSupport::Concern

  def create_successful_order!(order_data, order_id)
    order_values = base_order_values.merge(
      status: :success,
      offer_id: order_id,
      rate: order_data[:rate],
      amount: order_data[:amount],
      base: order_data[:base]
    )
    transactions.create!(order_values)
  end

  private

  def create_skipped_order!(order_data)
    order_values = base_order_values.merge(
      status: :skipped,
      rate: order_data[:rate],
      amount: order_data[:amount],
      base: order_data[:base]
    )
    transactions.create!(order_values)
  end

  def create_failed_order!(order_data, order_errors)
    order_values = base_order_values.merge(
      status: :failure,
      error_messages: order_errors,
      rate: order_data[:rate],
      amount: order_data[:amount],
      base: order_data[:base]
    )
    transactions.create!(order_values)
  end

  def base_order_values
    {
      quote: quote,
      bot_interval: interval,
      bot_price: quote_amount,
      transaction_type: 'REGULAR'
    }
  end
end
