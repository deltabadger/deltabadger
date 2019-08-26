module Api
  class BotsController < Api::BaseController
    def index
      render json: { data: Bot.all.select(:id, :state) }
    end

    def create
      bot_settings = bot_params.slice(:type, :price, :currency, :interval)
      bot = Bot.new(
        user: current_user,
        exchange_id: bot_params.fetch(:exchange_id),
        bot_type: 'free',
        settings: bot_settings
      )

      if bot.save
        render json: { data: true }, status: 201
      else
        render json: { data: false }, status: 422
      end
    end

    private

    def bot_params
      params
        .require(:bot)
        .permit(:exchange_id, :type, :price, :currency, :interval)
    end
  end
end
