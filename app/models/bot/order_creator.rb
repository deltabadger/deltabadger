module Bot::OrderCreator
  extend ActiveSupport::Concern

  def create_submitted_order!(order_data)
    order_values = base_order_values(order_data).merge(
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

  # Persist a durable record the instant the exchange accepts the order. Execution
  # amounts are unknown until Bot::FetchAndUpdateOrderJob confirms them. Idempotent:
  # never duplicate a row for an external_id we already have — on THIS venue. A merged bot inherits
  # rows placed on other venues, and an id is only unique per venue: an inherited row must never be
  # taken for the order just accepted here (the table's global unique index then refuses the insert
  # loudly, which beats recording a fill against the wrong row).
  def persist_accepted_order!(order_data, order_id)
    transactions.find_by(exchange_id:, external_id: order_id) ||
      create_submitted_order!(order_data.merge(order_id: order_id, status: :unknown))
  end

  def create_failed_order!(order_data)
    order_values = base_order_values(order_data).merge(
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
    order_values = base_order_values(order_data).merge(
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

  # A rebalance order is not a contribution — it must not read as one. `transaction_type` is what
  # separates them everywhere downstream: contribution accounting (Accountable, QuoteAmountLimitable)
  # and Bot#last_transaction all scope to 'REGULAR'. Threaded here rather than at each creator so
  # submitted, failed and skipped rows can never disagree about what an order was.
  def base_order_values(order_data = {})
    { bot_interval: interval, bot_quote_amount: quote_amount }.merge(order_identity_values(order_data))
  end

  # What every order row carries, whatever placed it. Kept apart from the schedule's two columns so a
  # bot with no schedule (Bots::Signal) takes all of this and none of that — and so a column added
  # here reaches every bot type, instead of being missed by an override that lists them again.
  # The assets are the ticker's: exact, where the symbols are what was shown.
  def order_identity_values(order_data = {})
    {
      transaction_type: order_data[:transaction_type].presence || 'REGULAR',
      exchange: exchange,
      base_asset_id: order_data[:ticker]&.base_asset_id,
      quote_asset_id: order_data[:ticker]&.quote_asset_id
    }
  end
end
