# frozen_string_literal: true

class SyncTrackerTool < ApplicationMCPTool
  tool_name 'sync_tracker'
  description 'Refresh the portfolio tracker: pull new account transactions and balances from every ' \
              'connected exchange. Runs in the background.'

  def perform
    result = BotApi::Tracker::Sync.call(user: current_user)
    return render(text: result.error_message) unless result.success?

    render text: "Sync started for #{result.data[:exchanges].join(', ')}. " \
                 'New transactions appear in list_account_transactions once it finishes.'
  end
end
