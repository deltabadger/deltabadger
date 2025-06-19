module Bot::Fundable
  extend ActiveSupport::Concern

  included do
    decorators = Module.new do
      def execute_action
        result = super
        return result if result.failure?

        if funds_are_low? && !notified_in_last_day?
          update!(last_end_of_funds_notification: Time.current)
          notify_end_of_funds
        end
        result
      end
    end

    prepend decorators
  end

  def funds_are_low?
    result = with_api_key do
      exchange.get_balance(asset_id: quote_asset_id)
    end
    return false if result.failure?

    quote_balance = result.data
    quote_balance[:free] < required_balance_buffer
  end

  private

  def notified_in_last_day?
    last_end_of_funds_notification.present? && last_end_of_funds_notification > 1.day.ago
  end

  def required_balance_buffer
    quote_amount / interval_duration.to_f * 3.days.to_f
  end
end
