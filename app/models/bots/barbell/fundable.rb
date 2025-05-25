module Bots::Barbell::Fundable
  extend ActiveSupport::Concern

  def notify_if_funds_are_low
    result = nil
    with_api_key do
      result = exchange.get_balance(asset_id: quote_asset_id)
    end
    return unless result.success?

    quote_balance = result.data
    return unless quote_balance[:free] >= quote_amount && quote_balance[:free] < quote_amount + required_balance_buffer

    notify_end_of_funds
  end

  private

  def required_balance_buffer
    quote_amount * (3.days.to_f / interval_duration)
  end
end
