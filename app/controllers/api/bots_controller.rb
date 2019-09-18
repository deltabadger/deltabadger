module Api
  class BotsController < Api::BaseController
    def index
      present_bot = lambda do |bot|
        Presenters::Api::Bot.call(bot)
      end

      data = BotsRepository.new.for_user(current_user).map(&present_bot)

      render json: { data: data }
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

    def start
      result = StartBot.call(params[:id])

      if result.success?
        render json: { data: true }, status: 200
      else
        render json: { data: false }, status: 422
      end
    end

    def stop
      result = StopBot.call(params[:id])

      if result.success?
        render json: { data: true }, status: 200
      else
        render json: { data: false }, status: 422
      end
    end

    def destroy
      result = RemoveBot.call(bot_id: params[:id], user: current_user)

      if result.success?
        render json: { data: true }, status: 200
      else
        render json: { errors: result.errors }, status: 422
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
