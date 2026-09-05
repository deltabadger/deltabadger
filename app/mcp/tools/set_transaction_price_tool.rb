# frozen_string_literal: true

class SetTransactionPriceTool < ApplicationMCPTool
  tool_name 'set_transaction_price'
  description 'State the USD price of one account transaction the venue did not value (a deposit, a gift, ' \
              'an airdrop). Empty clears it. Tiles, chart and tax reports use it.'

  property :transaction_id, type: 'number', required: true, description: 'Account transaction ID'
  property :price_usd, type: 'string', required: true,
                       description: "Price per unit in USD. Pass '' (empty) to clear a stated price."

  def perform
    result = BotApi::Tracker::SetTransactionPrice.call(user: current_user, transaction_id: transaction_id,
                                                       price_usd: price_usd)
    return render(text: result.error_message) unless result.success?

    text = if result.data[:price_usd]
             "Transaction ##{result.data[:id]} priced at #{result.data[:price_usd]} USD."
           else
             "Stated price cleared on transaction ##{result.data[:id]}."
           end
    render text: text
  end
end
