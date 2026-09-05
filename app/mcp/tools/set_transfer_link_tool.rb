# frozen_string_literal: true

class SetTransferLinkTool < ApplicationMCPTool
  tool_name 'set_transfer_link'
  description 'Mark a withdrawal and a deposit as one transfer between your own accounts (so it is not a ' \
              'disposal), or undo that. Give either side; the match is found within 14 days.'

  property :transaction_id, type: 'number', required: true,
                            description: 'Account transaction ID (from list_account_transactions)'
  property :linked, type: 'boolean', required: true, description: 'true to link, false to unlink'

  def perform
    result = BotApi::Tracker::SetTransferLink.call(user: current_user, transaction_id: transaction_id, linked: linked)
    return render(text: result.error_message) unless result.success?

    d = result.data
    text = if d[:linked]
             "Linked withdrawal ##{d[:withdrawal_id]} with deposit ##{d[:deposit_id]} as a transfer."
           else
             "Unlinked withdrawal ##{d[:withdrawal_id]} from deposit ##{d[:deposit_id]}."
           end
    render text: text
  end
end
