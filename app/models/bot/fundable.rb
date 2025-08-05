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
    result = get_balance(asset_id: quote_asset_id)
    return false if result.failure?

    quote_balance = result.data
    quote_balance[:free] < required_balance_buffer
  end

  private

  def notified_in_last_day?
    # # notified_in_last_day? per bot
    # last_end_of_funds_notification.present? && last_end_of_funds_notification > 1.day.ago

    # notified_in_last_day? per asset
    legacy_buy_bots = user.bots
                          .basic
                          .where('settings @> ?', { type: 'buy' }.to_json)
                          .where('settings @> ?', { quote: quote_asset.symbol }.to_json)
                          .pluck(:last_end_of_funds_notification)
    legacy_sell_bots = user.bots
                           .basic
                           .where('settings @> ?', { type: 'sell' }.to_json)
                           .where('settings @> ?', { base: quote_asset.symbol }.to_json)
                           .pluck(:last_end_of_funds_notification)
    new_bots = user.bots
                   .not_legacy
                   .where('settings @> ?', { quote_asset_id: quote_asset_id }.to_json)
                   .pluck(:last_end_of_funds_notification)
    (legacy_buy_bots + legacy_sell_bots + new_bots).compact.any? { |t| t > 1.day.ago }
  end

  def required_balance_buffer
    quote_amount / interval_duration.to_f * 3.days.to_f
  end
end
