module Api
  # rubocop:disable Metrics/ClassLength
  class BotsController < Api::BaseController
    include ActionController::Live

    skip_before_action :authenticate_user!, only: [:webhook]
    skip_before_action :verify_authenticity_token, only: [:webhook]

    def index
      bots = current_user
             .bots
             .not_deleted
             .legacy
             .includes(:exchange)
             .includes(:daily_transaction_aggregates)
             .includes(:transactions)
             .order(created_at: :desc)
             .page(params[:page])
      data = present_bots(bots)

      render json: { data: data }
    end

    def show
      bot = Bot.find(params[:id])
      data = present_bot(bot)

      render json: { data: data }
    end

    def create
      result = BotsManager::Create.call(current_user, bot_create_params)

      if result.success?
        render json: { data: result.data }, status: 201
      else
        render json: { errors: result.errors }, status: 422
      end
    end

    def update
      result = BotsManager::Update.call(current_user, bot_update_params)

      if result.success?
        data = present_bot(result.data)
        render json: { data: data }, status: 201
      else
        render json: { id: params[:id], errors: result.errors }, status: 422
      end
    end

    def webhook
      bot = Bot.find_by_webhook(params[:webhook])
      return render json: { errors: 'No bot found' }, status: 422 unless bot
      unless bot.possible_to_call_a_webhook?(params[:webhook])
        return render json: { errors: 'This webhook has already been called before' }, status: 422
      end

      ScheduleWebhook.call(bot, params[:webhook])
      render json: { result: 'Webhook invoked' }, status: 200
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

    def subaccounts
      result = GetSubaccounts.call(current_user, params[:exchange_id])

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

    def webhook_bots_data
      response.headers['Content-Type'] = 'text/event-stream'
      response.headers['Last-Modified'] = '0' # HACK: to bypass ETag caching
      response.headers['ETag'] = '0' # HACK: to bypass ETag caching
      sse = SSE.new(response.stream, retry: 300)
      last_updated_at = Time.current

      loop do
        newly_transactions = current_user.newly_webhook_bots_transactions(last_updated_at)
        if newly_transactions.present?
          bots = newly_transactions.map(&:bot).uniq
          bots.each { |bot| sse.write(present_webhook_bot(bot)) }
          last_updated_at = Time.current
        end

        sleep 3
      end
    rescue StandardError => e
      logger.info 'Stream closed'
      logger.info e
      response.stream.close
      sse.close
    ensure
      response.stream.close
      sse.close
    end

    private

    def bot_create_params
      @bot_create_params ||= case params[:bot][:bot_type]
                             when 'trading' then trading_bot_create_params
                             when 'withdrawal' then withdrawal_bot_create_params
                             else webhook_bot_create_params
                             end
    end

    def bot_update_params
      @bot_update_params ||= case params[:bot][:bot_type]
                             when 'trading' then trading_bot_update_params
                             when 'withdrawal' then withdrawal_bot_update_params
                             else webhook_bot_update_params
                             end
    end

    def present_bot(bot)
      return Presenters::Api::TradingBot.call(bot) if bot.basic?
      return Presenters::Api::WithdrawalBot.call(bot) if bot.withdrawal?

      Presenters::Api::WebhookBot.call(bot)
    end

    def present_webhook_bot(bot)
      Presenters::Api::WebhookBot.call(bot)
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
      use_subaccount
      selected_subaccount
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

    WEBHOOK_BOT_PARAMS = %i[
      exchange_id
      type
      price
      base
      quote
      bot_type
      name
      additional_type_enabled
      additional_type
      additional_price
      trigger_possibility
      order_type
    ].freeze

    def webhook_bot_create_params
      params
        .require(:bot)
        .permit(*WEBHOOK_BOT_PARAMS)
    end

    TRADING_BOT_UPDATE_PARAMS = %i[
      order_type
      price
      percentage
      interval
      force_smart_intervals
      smart_intervals_value
      price_range_enabled
      use_subaccount
      selected_subaccount
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

    WEBHOOK_BOT_UPDATE_PARAMS = %i[
      type
      price
      base
      quote
      bot_type
      name
      additional_type_enabled
      additional_type
      additional_price
      trigger_possibility
      order_type
    ].freeze

    def webhook_bot_update_params
      params
        .require(:bot)
        .permit(*WEBHOOK_BOT_UPDATE_PARAMS, already_triggered_types: [])
        .merge(id: params[:id])
    end

    def bot_continue_params
      params
        .require(:continue_params)
        .permit(:continue_schedule, :price)
    end
  end
  # rubocop:enable Metrics/ClassLength
end
