class AddApiKey < BaseService
  def initialize(
    api_key_validator: ApiKeyValidator.new,
    api_keys_repository: ApiKeysRepository.new
  )

    @api_key_validator = api_key_validator
    @api_keys_repository = api_keys_repository
  end

  def call(params)
    api_key = ApiKey.new(params)

    result = @api_key_validator.call(api_key)

    if result.success?
      saved_api_key = @api_keys_repository.save(api_key)
      Result::Success.new(saved_api_key)
    else
      Result::Failure.new(*result.errors)
    end
  end
end
