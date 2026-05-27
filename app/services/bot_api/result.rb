# frozen_string_literal: true

module BotApi
  # Shared structured-result envelope for service-layer calls. `status` is a
  # domain symbol, not HTTP-coupled. Surface layers map it as they need:
  #
  #   :success            → 200
  #   :created            → 201
  #   :validation_failed  → 422
  #   :permission_denied  → 403
  #   :not_found          → 404
  #   :upstream_failed    → 502
  #   :conflict           → 409
  #
  # MCP wrappers ignore `status` and present text from `data` / `error_message`.
  Result = Data.define(:status, :data, :error_code, :error_message) do
    def success? = error_code.nil?

    def self.success(data, status: :success)
      new(status: status, data: data, error_code: nil, error_message: nil)
    end

    def self.failure(status, error_code, error_message, data: nil)
      new(status: status, data: data, error_code: error_code.to_s, error_message: error_message)
    end
  end
end
