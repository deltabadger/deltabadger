class ApiKeyValidator < BaseService
  def initialize(get_validator: ExchangeApi::Clients::GetValidator.new)
    @get_validator = get_validator
  end

  def call(api_key)
    validator = @get_validator.call(api_key)
    return Result::Failure.new('Invalid tokens') unless api_key.valid?
    return Result::Failure.new('Invalid tokens') unless validator.validate_credentials(
      api_key: api_key.key,
      api_secret: api_key.secret
    )

    Result::Success.new
  end
end
