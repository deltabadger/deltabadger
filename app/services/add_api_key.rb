class AddApiKey < BaseService
  def call(params)
    api_key = ApiKey.create!(params.merge(status: 'pending_validation'))
    ApiKeyValidatorJob.perform_later(api_key.id)

    Result::Success.new(api_key)
  rescue ActiveRecord::RecordInvalid => e
    # Legacy POST /api/api_keys calls straight through to here, so an unrescued RecordInvalid
    # surfaces as a 500 instead of the 422 that endpoint already knows how to render.
    Result::Failure.new(e.record.errors.full_messages)
  end
end
