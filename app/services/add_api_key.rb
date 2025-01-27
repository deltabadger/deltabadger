class AddApiKey < BaseService
  API_KEYS_VALIDATION_QUEUE = 'api_keys_validation'.freeze
  def initialize(
    validator_worker: ApiKeyValidatorWorker
  )
    @validator_worker = validator_worker
  end

  def call(params)
    api_key = ApiKey.create!(params.merge(status: 'pending'))
    @validator_worker.sidekiq_options(queue: API_KEYS_VALIDATION_QUEUE)
    @validator_worker.perform_at(
      Time.now,
      api_key.id
    )

    Result::Success.new(api_key)
  end
end
