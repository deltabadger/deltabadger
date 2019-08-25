module Api
  class ApiKeysController < ApplicationController
    before_action :authenticate_user!

    def create
      api_key = ApiKey.new(
        api_key_params.merge(user: current_user)
      )

      if api_key.save!
        render json: true
      else
        render json: false
      end
    end

    private

    def api_key_params
      params.require(:api_key).permit(:key, :exchange_id)
    end
  end
end
