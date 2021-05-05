module Api
  class ApiKeysController < Api::BaseController
    def create
      result = AddApiKey.call(
        api_key_params.merge(user: current_user)
      )

      if result.success?
        render json: { data: true }, status: 201
      else
        render json: { data: false }, status: 422
      end
    end

    def revalidate_api_key
      exchange_id = revalidate_params
      api_key = current_user.api_keys.find_by(exchange_id: exchange_id)
      api_key.update(status: 'pending')

      ApiKeyValidatorWorker.perform_at(
        Time.now,
        api_key.id
      )

      render json: { data: true }, status: 200
    end

    def remove
      exchange_id = remove_params
      api_key = current_user.api_keys.find_by(exchange_id: exchange_id)
      api_key.destroy!

      render json: { data: true }, status: 200
    end

    private

    def api_key_params
      params.require(:api_key).permit(:key, :secret, :passphrase, :exchange_id, :german_trading_agreement)
    end

    def revalidate_params
      params.require(:exchange_id)
    end

    def remove_params
      params.require(:exchange_id)
    end
  end
end
