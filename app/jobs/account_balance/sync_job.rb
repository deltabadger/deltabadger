class AccountBalance::SyncJob < ApplicationJob
  include ApiKeyFailureHandling

  queue_as :low_priority
  limits_concurrency to: 1, key: ->(user_id, *) { "sync_balances_#{user_id}" }, on_conflict: :discard

  def perform(user_id, api_key_ids)
    api_keys = ApiKey.where(id: api_key_ids, status: :correct, key_type: :trading).includes(:exchange)
    return if api_keys.empty?

    pricing_errors = []
    api_keys.each do |api_key|
      err = sync(api_key)
      pricing_errors << err if err
    end

    broadcast_pricing_warning(user_id, pricing_errors.first) if pricing_errors.any?
    broadcast_refresh(user_id)
  end

  private

  # Returns pricing_error string if live pricing fully failed for this key, else nil.
  def sync(api_key)
    result = AccountBalance::Sync.new(api_key).sync!
    handle_api_key_failure(api_key, result)
    return nil unless result.success?

    summary = result.data
    summary.pricing_fully_failed? ? summary.pricing_error : nil
  rescue StandardError => e
    Rails.logger.error("[AccountBalance::SyncJob] #{api_key.exchange.name} failed: #{e.message}")
    nil
  end

  def broadcast_pricing_warning(user_id, message)
    Turbo::StreamsChannel.broadcast_append_to(
      "user_#{user_id}", :sync,
      target: 'flash',
      partial: 'tracker/pricing_warning',
      locals: { message: message }
    )
  end

  def broadcast_refresh(user_id)
    Turbo::StreamsChannel.broadcast_refresh_to("user_#{user_id}", :sync)
  end
end
