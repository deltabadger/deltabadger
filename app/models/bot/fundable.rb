module Bot::Fundable
  extend ActiveSupport::Concern

  included do
    decorators = Module.new do
      def execute_action
        result = super
        return result if result.failure?

        # While selling, the user spends base, not quote — the quote-buffer check is meaningless
        # and would fire spurious "out of funds" notices. Skip it (and the live balance call).
        if !try(:selling?) && funds_are_low? && !notified_in_last_day?
          update!(last_end_of_funds_notification: Time.current)
          notify_end_of_funds
        end
        result
      end
    end

    prepend decorators
  end

  def funds_are_low?
    result = get_balance(asset_id: quote_asset_id)
    return false if result.failure?

    # Not the settled balance: on a margin venue a bot spends buying power, and which figure
    # applies depends on what it trades, so the exchange picks.
    exchange.spendable_balance(result.data, tickers: tickers) < required_balance_buffer
  end

  private

  def notified_in_last_day?
    # notified_in_last_day? per asset - check all bots with the same quote_asset
    user.bots
        .where("json_extract(settings, '$.quote_asset_id') = ?", quote_asset_id)
        .where.not(last_end_of_funds_notification: nil)
        .where('last_end_of_funds_notification > ?', 1.day.ago)
        .exists?
  end

  def required_balance_buffer
    quote_amount / interval_duration.to_f * 3.days.to_f
  end
end
