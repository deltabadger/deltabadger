module Api
  class BotsController < Api::BaseController
    def index
      bots = BotsRepository.new.for_user(current_user, params[:page])
      data = present_bots(bots)

      render json: { data: data }
    end

    def show
      bot = BotsRepository.new.by_id_for_user(current_user, params[:id])
      data = present_bot(bot)

      render json: { data: data }
    end

    def create
      bot_params = params[:bot][:bot_type] == 'free' ? trading_bot_create_params : withdrawal_bot_create_params
      result = Bots::Create.call(current_user, bot_params)

      if result.success?
        render json: { data: result.data }, status: 201
      else
        render json: { errors: result.errors }, status: 422
      end
    end

    def update
      bot_params = params[:bot][:order_type].present? ? trading_bot_update_params : withdrawal_bot_update_params
      result = Bots::Update.call(current_user, bot_params)

      if result.success?
        data = present_bot(result.data)
        render json: { data: data }, status: 201
      else
        render json: { id: params[:id], errors: result.errors }, status: 422
      end
    end

    def start
      result = StartBot.call(params[:id], bot_continue_params)

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

    def restart_params
      result = GetRestartParams.call(bot_id: params[:bot_id])

      render json: result, status: 200
    end

    def smart_intervals_info
      result = GetSmartIntervalsInfo.call(params, current_user)

      if result.success?
        render json: result, status: 200
      else
        render json: { errors: result.errors }, status: 422
      end
    end

    def withdrawal_minimums
      result = GetWithdrawalMinimums.call(params, current_user)

      if result.success?
        render json: result, status: 200
      else
        render json: { errors: result.errors }, status: 422
      end
    end

    def frequency_limit_exceeded
      limit_exceeded = CheckExceededFrequency.call(params)
      render json: limit_exceeded.to_json
    end

    def set_show_smart_intervals_info
      GetSmartIntervalsInfo.new.set_show_smart_intervals(current_user)
    end

    def continue
      result = StartBot.call(params[:id], bot_continue_params[:continue_schedule])

      if result.success?
        data = present_bot(result.data)
        render json: { data: data }, status: 200
      else
        render json: { id: params[:id], errors: result.errors }, status: 422
      end
    end

    private

    def present_bot(bot)
      bot.trading? ? Presenters::Api::TradingBot.call(bot) : Presenters::Api::WithdrawalBot.call(bot)
    end

    def present_bots(bots)
      Presenters::Api::Bots.call(bots)
    end

    TRADING_BOT_PARAMS = %i[
      exchange_id
      type
      order_type
      price
      percentage
      base
      quote
      interval
      bot_type
      force_smart_intervals
      smart_intervals_value
      price_range_enabled
    ].freeze

    def trading_bot_create_params
      params
        .require(:bot)
        .permit(*TRADING_BOT_PARAMS, price_range: [])
    end

    WITHDRAWAL_BOT_PARAMS = %i[
      exchange_id
      interval
      interval_enabled
      threshold
      threshold_enabled
      currency
      address
      bot_type
    ].freeze

    def withdrawal_bot_create_params
      params
        .require(:bot)
        .permit(*WITHDRAWAL_BOT_PARAMS)
    end

    TRADING_BOT_UPDATE_PARAMS = %i[
      order_type
      price
      percentage
      interval
      force_smart_intervals
      smart_intervals_value
      price_range_enabled
    ].freeze

    def trading_bot_update_params
      params
        .require(:bot)
        .permit(*TRADING_BOT_UPDATE_PARAMS, price_range: [])
        .merge(id: params[:id])
    end

    WITHDRAWAL_BOT_UPDATE_PARAMS = %i[
      interval
      interval_enabled
      threshold
      threshold_enabled
    ].freeze

    def withdrawal_bot_update_params
      params
        .require(:bot)
        .permit(*WITHDRAWAL_BOT_UPDATE_PARAMS)
        .merge(id: params[:id])
    end

    def bot_continue_params
      params
        .require(:continue_params)
        .permit(:continue_schedule, :price)
    end
  end
end
