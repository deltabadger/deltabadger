class ApiKeyValidator < BaseService
  def initialize(get_validator: ExchangeApi::Validators::Get.new)
    @get_validator = get_validator
  end

  def call(api_key)
    validator = @get_validator.call(api_key.exchange_id)
    return Result::Failure.new('Invalid tokens') unless api_key.valid?
    return Result::Failure.new('Invalid tokens') unless validator.validate_credentials(get_params(api_key))

    Result::Success.new
  end

  private

  def get_params(api_key)
    params = {
      api_key: api_key.key,
      api_secret: api_key.secret
    }
    params = params.merge({passphrase: api_key.passphrase}) if api_key.passphrase != ''

    params
  end
end
