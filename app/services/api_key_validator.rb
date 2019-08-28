class ApiKeyValidator < BaseService
  # def initialize(exchange_api: ExchangeApi.new)
  #   @exchange_api = exchange_api
  # end

  def call(api_key)
    return Result::Failure.new('Invalid params') if !api_key.valid?

    Result::Success.new
  end
end
