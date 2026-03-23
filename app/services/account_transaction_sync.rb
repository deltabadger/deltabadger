class AccountTransactionSync
  def initialize(api_key)
    @api_key = api_key
    @exchange = api_key.exchange
  end

  def sync!
    result = @exchange.get_ledger(api_key: @api_key, start_time: @api_key.last_synced_at)
    return result if result.failure?

    imported = 0
    result.data.each do |entry|
      next if entry[:tx_id].present? && AccountTransaction.exists?(exchange: @exchange, tx_id: entry[:tx_id])

      at = AccountTransaction.new(
        api_key: @api_key,
        exchange: @exchange,
        entry_type: entry[:entry_type],
        base_currency: entry[:base_currency],
        base_amount: entry[:base_amount],
        quote_currency: entry[:quote_currency],
        quote_amount: entry[:quote_amount],
        fee_currency: entry[:fee_currency],
        fee_amount: entry[:fee_amount],
        tx_id: entry[:tx_id],
        group_id: entry[:group_id],
        description: entry[:description],
        transacted_at: entry[:transacted_at],
        raw_data: entry[:raw_data] || {}
      )

      match_bot_transaction!(at) if at.buy? || at.sell? || at.swap_in? || at.swap_out?
      at.save!
      imported += 1
    end

    @api_key.update!(last_synced_at: Time.current)
    Result::Success.new(imported)
  end

  private

  def match_bot_transaction!(at)
    return unless at.tx_id.present?

    bot_tx = Transaction.find_by(external_id: at.tx_id, exchange: @exchange)
    at.bot_transaction = bot_tx if bot_tx
  end
end
