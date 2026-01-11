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

  def import_orders_csv(file)
    csv_content = file.read
    rows = CSV.parse(csv_content, headers: true)

    # Validate CSV structure
    expected_headers = ['Timestamp', 'Order ID', 'Type', 'Side', 'Amount', 'Value', 'Price', 'Base Asset', 'Quote Asset', 'Status']
    unless rows.headers == expected_headers
      return { success: false, error: I18n.t('bot.details.stats.import_invalid_format') }
    end

    # Get bot's currencies - handle both single and dual asset bots
    bot_base_symbols = if respond_to?(:base_asset) && base_asset.present?
                         [base_asset.symbol&.upcase]
                       elsif respond_to?(:base0_asset) && respond_to?(:base1_asset)
                         [base0_asset&.symbol&.upcase, base1_asset&.symbol&.upcase].compact
                       else
                         []
                       end
    bot_quote = quote_asset&.symbol&.upcase

    imported_count = 0
    existing_order_ids = transactions.pluck(:external_id).compact

    rows.each do |row|
      csv_base = row['Base Asset']&.upcase
      csv_quote = row['Quote Asset']&.upcase

      # Skip rows where currencies don't match (base must be one of the bot's base assets, quote must match)
      next unless bot_base_symbols.include?(csv_base) && csv_quote == bot_quote

      order_id = row['Order ID']

      # Skip if order already exists
      next if existing_order_ids.include?(order_id)

      # Parse values
      timestamp = Time.zone.parse(row['Timestamp'])
      order_type = "#{row['Type'].downcase}_order"
      side = row['Side'].downcase
      amount = row['Amount'].to_d
      value = row['Value'].to_d
      price = row['Price'].to_d
      status = row['Status'].downcase

      # Create the transaction
      transactions.create!(
        exchange: exchange,
        external_id: order_id,
        created_at: timestamp,
        order_type: order_type,
        side: side,
        amount: amount,
        amount_exec: amount,
        quote_amount: value,
        quote_amount_exec: value,
        price: price,
        base: csv_base,
        quote: csv_quote,
        status: :submitted,
        external_status: status == 'closed' ? :closed : (status == 'open' ? :open : :unknown)
      )

      imported_count += 1
    end

    { success: true, imported_count: imported_count }
  rescue CSV::MalformedCSVError => e
    { success: false, error: I18n.t('bot.details.stats.import_malformed_csv') }
  rescue StandardError => e
    { success: false, error: I18n.t('bot.details.stats.import_error', message: e.message) }
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
