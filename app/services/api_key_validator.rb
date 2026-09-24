class ApiKeyValidator < BaseService
  # The same check as every form: ApiKey#get_validity, which names what a venue reports missing.
  def call(api_key_id)
    api_key = ApiKey.find(api_key_id)

    unless api_key.valid?
      api_key.update(status: 'incorrect')
      return Result::Failure.new(I18n.t('errors.invalid_api_keys'))
    end

    api_key.update_status!(api_key.get_validity)
    return Result::Failure.new(I18n.t('errors.invalid_api_keys')) unless api_key.correct?

    enqueue_balance_sync(api_key)
    Result::Success.new
  end

  def enqueue_balance_sync(api_key)
    return unless api_key.key_type == 'trading'

    AccountBalance::SyncJob.perform_later(api_key.user_id, [api_key.id])
  end
end
