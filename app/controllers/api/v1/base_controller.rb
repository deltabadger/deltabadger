# frozen_string_literal: true

module Api
  module V1
    # OAuth-only base for the versioned REST API. Per-action tool gating
    # uses `current_user.rest_tool_enabled?(tool_name)`. Existing
    # session-auth `Api::BaseController` is unaffected and continues to
    # serve `/api/api_keys` and `/api/exchanges`.
    class BaseController < ActionController::API
      include ApiOauthAuthentication

      private

      def require_rest_tool!(tool_name)
        return if current_user.rest_tool_enabled?(tool_name)

        render json: {
          data: nil,
          error: { code: 'tool_disabled', message: "Tool '#{tool_name}' is disabled for this user." }
        }, status: :forbidden
      end

      def render_result(result, success_status: nil)
        if result.success?
          render json: { data: result.data, error: nil },
                 status: success_status || status_for(result.status)
        else
          render json: {
            data: nil,
            error: { code: result.error_code, message: result.error_message }
          }, status: status_for(result.status)
        end
      end

      def status_for(domain_status)
        {
          success: :ok,
          created: :created,
          validation_failed: :unprocessable_entity,
          permission_denied: :forbidden,
          not_found: :not_found,
          upstream_failed: :bad_gateway,
          conflict: :conflict
        }.fetch(domain_status, :internal_server_error)
      end
    end
  end
end
