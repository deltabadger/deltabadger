class ApiKeyValidator < BaseService
  def initialize(
    get_validator: ExchangeApi::Validators::Get.new,
    api_keys_repository: ApiKeysRepository.new
  )

    @get_validator = get_validator
    @api_keys_repository = api_keys_repository
  end

  def call(api_key_id)
    api_key = @api_keys_repository.find(api_key_id)
    validator = @get_validator.call(api_key.exchange_id, api_key.key_type)

    unless api_key.valid? && validator.validate_credentials(get_params(api_key))
      @api_keys_repository.update(api_key.id, status: 'incorrect')
      return Result::Failure.new(I18n.t('errors.invalid_api_keys'))
    end

    @api_keys_repository.update(api_key.id, status: 'correct')
    Result::Success.new
  end

  private

  def get_params(api_key)
    params = {
      api_key: api_key.key,
      api_secret: api_key.secret
    }
    params = params.merge(passphrase: api_key.passphrase) if api_key.passphrase.present?

    params
  end
end
