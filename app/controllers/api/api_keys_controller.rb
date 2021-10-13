module Api
  class ApiKeysController < Api::BaseController
    def create
      byebug
      keys_params = api_key_params.merge(user: current_user)
      api_key = current_user.api_keys
                            .find_by(exchange_id: keys_params[:exchange_id], key_type: keys_params[:type])

      result = if api_key.nil?
                 AddApiKey.call(keys_params)
               elsif same_keys?(keys_params, api_key)
                 revalidate_api_key(api_key)
               else
                 remove(api_key)
                 AddApiKey.call(keys_params)
               end

      if result.success?
        render json: { data: true }, status: 201
      else
        render json: { data: false }, status: 422
      end
    end

    def remove_invalid_keys
      api_key = current_user.api_keys.find_by(exchange_id: invalid_key_params[:exchange_id])
      return if api_key.nil? || !api_key.incorrect?

      remove(api_key)
    end

    private

    def revalidate_api_key(api_key)
      api_key.update(status: 'pending')

      ApiKeyValidatorWorker.perform_at(
        Time.now,
        api_key.id
      )

      Result::Success.new
    rescue StandardError
      Result::Failure.new
    end

    def remove(api_key)
      api_key.destroy!
    end

    def api_key_params
      params.require(:api_key).permit(:key, :secret, :passphrase, :exchange_id, :german_trading_agreement, :type)
    end

    def invalid_key_params
      params.permit(:exchange_id)
    end

    def same_keys?(params, api_key)
      params[:key] == api_key.key && params[:secret] == api_key.secret
    end
  end
end
