class ApiKeyValidatorWorker
  include Sidekiq::Worker

  def perform(api_key_id)
    ApiKeyValidator.call(api_key_id)
  rescue StandardError => e # prevent job from retrying
    Raven.capture_exception(e)
  end
end
