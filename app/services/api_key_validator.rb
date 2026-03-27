class ApiKeyValidator < BaseService
  def call(api_key_id)
    api_key = ApiKey.find(api_key_id)

    unless api_key.valid? && validate(api_key)
      api_key.update(status: 'incorrect')
      return Result::Failure.new(I18n.t('errors.invalid_api_keys'))
    end

    api_key.update(status: 'correct')
    Result::Success.new
  end

  private

  def validate(api_key)
    return true if Rails.configuration.dry_run

    exchange_name = api_key.exchange.name_id
    client_params = { api_key: api_key.key, api_secret: api_key.secret }
    client_params[:passphrase] = api_key.passphrase if api_key.passphrase.present?

    result = Honeymaker.client(exchange_name, **client_params).validate(:trading)
    result.success?
  rescue StandardError
    false
  end
end
