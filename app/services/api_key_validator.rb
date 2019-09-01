class ApiKeyValidator < BaseService
  def initialize(exchange_api: ExchangeApi::Get.new)
    @get_exchange_api = exchange_api
  end

  def call(api_key)
    api = @get_exchange_api.call(api_key)
    return Result::Failure.new('Invalid tokens') if !api_key.valid?
    return Result::Failure.new('Invalid tokens') if !api.validate_credentials

    Result::Success.new
  end
end
