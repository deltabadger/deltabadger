class AddApiKey < BaseService
  API_KEYS_VALIDATION_QUEUE = 'api_keys_validation'.freeze
  def initialize(
    validator_worker: ApiKeyValidatorWorker
  )
    @validator_worker = validator_worker
  end

  def call(params)
    saved_api_key = ApiKey.save(ApiKey.new(params.merge(status: 'pending')))
    @validator_worker.sidekiq_options(queue: API_KEYS_VALIDATION_QUEUE)
    @validator_worker.perform_at(
      Time.now,
      saved_api_key.id
    )

    Result::Success.new(saved_api_key)
  end
end
