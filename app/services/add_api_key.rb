class AddApiKey < BaseService
  API_KEYS_VALIDATION_QUEUE = 'api_keys_validation'.freeze
  def initialize(
    api_keys_repository: ApiKeysRepository.new,
    validator_worker: ApiKeyValidatorWorker
  )

    @api_keys_repository = api_keys_repository
    @validator_worker = validator_worker
  end

  def call(params)
    saved_api_key = @api_keys_repository.save(ApiKey.new(params.merge(status: 'pending')))
    @validator_worker.sidekiq_options(queue: API_KEYS_VALIDATION_QUEUE)
    @validator_worker.perform_at(
      Time.now,
      saved_api_key.id
    )

    Result::Success.new(saved_api_key)
  end
end
