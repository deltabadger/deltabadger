require 'csv'

module Bot::Exportable
  extend ActiveSupport::Concern

  def orders_csv
    headers = [
      'Timestamp',
      'Order ID',
      'Type',
      'Side',
      'Amount',
      'Value',
      'Price',
      'Base Asset',
      'Quote Asset',
      'Status'
    ]

    transactions_data = transactions
                        .submitted
                        .order(:created_at)
                        .pluck(
                          :created_at,
                          :external_id,
                          :order_type,
                          :side,
                          :amount,
                          :amount_exec,
                          :quote_amount,
                          :quote_amount_exec,
                          :price,
                          :base,
                          :quote,
                          :external_status
                        )

    CSV.generate do |csv|
      csv << headers
      parsed_csv_values(transactions_data).each { |row| csv << row }
    end
  end

  private

  def parsed_csv_values(transactions_data)
    transactions_data.map do |created_at,
                              external_id,
                              order_type,
                              side,
                              amount,
                              amount_exec,
                              quote_amount,
                              quote_amount_exec,
                              price,
                              base,
                              quote,
                              external_status|
      [
        created_at.in_time_zone(user.time_zone),
        external_id,
        order_type.delete_suffix('_order').humanize.titleize,
        side.humanize.titleize,
        parse_amount(amount_exec, amount),
        parse_quote_amount(quote_amount_exec, quote_amount, amount, price),
        price,
        base.upcase,
        quote.upcase,
        external_status.humanize.titleize
      ]
    end
  end

  def parse_amount(amount_exec, amount)
    amount_exec || amount
  end

  def parse_quote_amount(quote_amount_exec, quote_amount, amount, price)
    quote_amount_exec || quote_amount || (amount * price if amount.present? && price.present? && price.positive?)
  end
end
