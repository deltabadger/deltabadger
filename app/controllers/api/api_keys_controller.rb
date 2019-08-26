module Api
  class ApiKeysController < Api::BaseController
    def create
      api_key = ApiKey.new(
        api_key_params.merge(user: current_user)
      )

      if api_key.save!
        render json: { data: true }, status: 201
      else
        render json: { data: false }, status: 422
      end
    end

    private

    def api_key_params
      params.require(:api_key).permit(:key, :exchange_id)
    end
  end
end
