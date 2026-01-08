class ApiKeyValidatorJob < ApplicationJob
  queue_as :api_keys_validation

  # Don't retry - validation should be immediate
  discard_on StandardError

  def perform(api_key_id)
    ApiKeyValidator.call(api_key_id)
  end
end
