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

    private

    def api_key_params
      params.require(:api_key).permit(:key, :secret, :exchange_id)
    end
  end
end
