class AddApiKey < BaseService
  def call(params)
    api_key = ApiKey.create!(params.merge(status: 'pending_validation'))
    ApiKeyValidatorJob.perform_later(api_key.id)

    Result::Success.new(api_key)
  end
end
