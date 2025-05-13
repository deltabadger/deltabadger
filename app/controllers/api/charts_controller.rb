module Api
  class ChartsController < Api::BaseController
    def portfolio_value_over_time
      bot = Bot.find(params[:bot_id])

      result = Charts::PortfolioValueOverTime::Chart.call(bot)

      if result.success?
        data = result.data.map do |date, total_invested, value|
          [date.in_time_zone(current_user.time_zone), total_invested, value]
        end
        render json: { data: data }, status: 200
      else
        render json: { errors: result.errors }, status: 422
      end
    end
  end
end
