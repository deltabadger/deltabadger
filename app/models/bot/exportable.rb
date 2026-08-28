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
                        .where(external_status: :closed)
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

    CsvSafe.generate do |csv|
      csv << headers
      parsed_csv_values(transactions_data).each { |row| csv << row }
    end
  end

  def import_orders_csv(file)
    csv_content = file.read
    rows = CSV.parse(csv_content, headers: true)

    # Validate CSV structure
    expected_headers = ['Timestamp', 'Order ID', 'Type', 'Side', 'Amount', 'Value', 'Price', 'Base Asset', 'Quote Asset',
                        'Status']
    return { success: false, error: I18n.t('bot.details.stats.import_invalid_format') } unless rows.headers == expected_headers

    bot_base_symbols = importable_base_symbols
    bot_quote = quote_asset&.symbol&.upcase

    skipped_currency_mismatch = 0
    skipped_already_exists = 0
    existing_order_ids = Set.new(transactions.pluck(:external_id).compact)
    now = Time.current
    records = []

    rows.each do |row|
      csv_base = CsvSafe.unescape(row['Base Asset'])&.strip&.upcase
      csv_quote = CsvSafe.unescape(row['Quote Asset'])&.strip&.upcase

      # Skip rows where currencies don't match
      # For index bots (bot_base_symbols is nil), accept any base asset as long as quote matches
      base_matches = bot_base_symbols.nil? || bot_base_symbols.include?(csv_base)
      unless base_matches && csv_quote == bot_quote
        skipped_currency_mismatch += 1
        next
      end

      original_order_id = CsvSafe.unescape(row['Order ID'])

      # Generate a unique external_id for the imported transaction
      # This allows importing the same orders to different bots
      order_id = "imported_#{id}_#{original_order_id}"

      # Skip if this order was already imported to THIS bot
      if existing_order_ids.include?(order_id)
        skipped_already_exists += 1
        next
      end

      # Only import closed/finalized orders
      status = CsvSafe.unescape(row['Status'])&.strip&.downcase
      next unless status == 'closed'

      # Parse values
      timestamp = Time.zone.parse(row['Timestamp'])
      order_type = "#{CsvSafe.unescape(row['Type']).downcase}_order"
      side = CsvSafe.unescape(row['Side']).downcase
      amount = row['Amount'].to_d
      value = row['Value'].to_d
      price = row['Price'].to_d

      records << {
        bot_id: id,
        exchange_id: exchange.id,
        external_id: order_id,
        created_at: timestamp,
        updated_at: now,
        order_type: order_type,
        side: side,
        amount: amount.round(18),
        amount_exec: amount.round(18),
        quote_amount: value.round(18),
        quote_amount_exec: value.round(18),
        price: price.round(18),
        base: csv_base,
        quote: csv_quote,
        status: :submitted,
        external_status: :closed
      }
    end

    # Provide detailed feedback
    if records.empty? && skipped_currency_mismatch.positive?
      return {
        success: false,
        error: I18n.t('bot.details.stats.import_currency_mismatch',
                      csv_quote: rows.first&.dig('Quote Asset'),
                      bot_quote: bot_quote)
      }
    end

    # Bulk insert — single query, skips callbacks (no per-row broadcasts/jobs)
    Transaction.insert_all(records) if records.any?

    # Single metrics update after all rows are imported
    Bot::UpdateMetricsJob.perform_later(self) if records.any?

    { success: true, imported_count: records.size, skipped_existing: skipped_already_exists }
  rescue CSV::MalformedCSVError
    { success: false, error: I18n.t('bot.details.stats.import_malformed_csv') }
  rescue StandardError => e
    { success: false, error: I18n.t('bot.details.stats.import_error', message: e.message) }
  end

  # Which base assets a CSV import may name. nil means "any", which is right for an index bot whose
  # membership follows the market and wrong for everything else.
  #
  # A basket answers with its own members. Without this branch a converted pair bot — which has
  # neither base_asset nor base0_asset — fell through to "any" and would silently accept rows for
  # assets it has never held.
  def importable_base_symbols
    return [base_asset.symbol&.upcase] if respond_to?(:base_asset) && base_asset.present?
    return [base0_asset&.symbol&.upcase, base1_asset&.symbol&.upcase].compact if respond_to?(:base0_asset)
    return base_assets.filter_map { |asset| asset.symbol&.upcase } if respond_to?(:allocations)

    nil
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
