module Api
  class BotsController < Api::BaseController
    def index
      data =
        BotsRepository.new.for_user(current_user).map(&method(:present_bot))

      render json: { data: data }
    end

    def show
      bot = BotsRepository.new.by_id_for_user(current_user, params[:id])
      data = present_bot(bot)

      render json: { data: data }
    end

    def create
      result = Bots::Create.call(current_user, bot_create_params)

      if result.success?
        render json: { data: result.data }, status: 201
      else
        render json: { errors: result.errors }, status: 422
      end
    end

    def update
      result = Bots::Update.call(current_user, bot_update_params)

      if result.success?
        data = present_bot(result.data)
        render json: { data: data }, status: 201
      else
        render json: { id: params[:id], errors: result.errors }, status: 422
      end
    end

    def start
      result = StartBot.call(params[:id])

      if result.success?
        data = present_bot(result.data)
        render json: { data: data }, status: 200
      else
        render json: { id: params[:id], errors: result.errors }, status: 422
      end
    end

    def stop
      result = StopBot.call(params[:id])

      if result.success?
        data = present_bot(result.data)
        render json: { data: data }, status: 200
      else
        render json: { id: params[:id], errors: result.errors }, status: 422
      end
    end

    def destroy
      result = RemoveBot.call(bot_id: params[:id], user: current_user)

      if result.success?
        render json: { data: true }, status: 200
      else
        render json: { id: params[:id], errors: result.errors }, status: 422
      end
    end

    private

    def present_bot(bot)
      Presenters::Api::Bot.call(bot)
    end

    BOT_PARAMS = %i[
      exchange_id
      type
      order_type
      price
      percentage
      base
      quote
      interval
      bot_type
      force
    ].freeze

    def bot_create_params
      params
        .require(:bot)
        .permit(*BOT_PARAMS)
    end

    def bot_update_params
      params
        .require(:bot)
        .permit(:order_type, :interval, :price, :percentage).merge(id: params[:id])
    end
  end
end
