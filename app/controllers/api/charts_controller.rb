module Api
  class ChartsController < Api::BaseController
    def portfolio_value_over_time
      bot = BotsRepository.new.by_id_for_user(current_user, params[:bot_id])

      result = Charts::PortfolioValueOverTime::Chart.call(bot)

      if result.success?
        render json: { data: result.data }, status: 200
      else
        render json: { errors: result.errors }, status: 422
      end
    end
  end
end
