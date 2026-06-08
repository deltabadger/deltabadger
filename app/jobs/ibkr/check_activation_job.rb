# Re-checks IBKR keys that are awaiting activation. IBKR activates a self-service OAuth consumer
# key on its weekend server restart (24h–2wk after registration). When a key becomes usable we
# flip it to :correct; bots parked on it resume automatically on their next ActionJob run (the
# pending-activation guard in Bot::ActionJob lets them through once the key is :correct).
class Ibkr::CheckActivationJob < ApplicationJob
  queue_as :low_priority

  def perform
    pending_ibkr_keys.find_each do |api_key|
      result = api_key.exchange.get_api_key_validity(api_key: api_key)
      next unless result.success? && result.data == true

      api_key.update!(status: :correct)
      Rails.logger.info("[IBKR] api_key=#{api_key.id} activated; parked bots will resume")
    rescue StandardError => e
      Rails.logger.warn("[IBKR] activation check failed for api_key=#{api_key.id}: #{e.message}")
    end
  end

  private

  def pending_ibkr_keys
    ApiKey.where(status: :pending_activation)
          .joins(:exchange).where(exchanges: { type: 'Exchanges::Ibkr' })
  end
end
