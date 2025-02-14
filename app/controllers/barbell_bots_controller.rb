class BarbellBotsController < ApplicationController
  before_action :authenticate_user!

  def index
    @bots = current_user.bots.not_deleted.barbell
  end

  def new
    @barbell_bot = current_user.bots.barbell.new
  end

  def create
    exchange_id = barbell_bot_params[:exchange_id]
    settings = barbell_bot_params.except(:exchange_id)
    @barbell_bot = current_user.bots.barbell.new(settings: settings, exchange_id: exchange_id)

    puts "barbell_bot: #{@barbell_bot.inspect}"

    if @barbell_bot.save
      redirect_to barbell_bot_path(@barbell_bot), notice: 'Bot created successfully'
    else
      puts "barbell_bot errors: #{@barbell_bot.errors.messages.inspect}"
      flash.now[:alert] = @barbell_bot.errors.messages.values.join(', ')
      render :new, status: :unprocessable_entity
    end
  end

  def show
    @barbell_bot = current_user.bots.barbell.find(params[:id])

    return if trader_key_id_for_user(@barbell_bot.exchange).present?

    @api_key = ApiKey.new(user: current_user, exchange: @barbell_bot.exchange)
    render :api_keys
  end

  # def update
  #   @barbell_bot.update(params.permit(:settings))
  #   render json: { success: true }
  # end

  def destroy
    @barbell_bot = current_user.bots.barbell.find(params[:barbell_bot_id])
    puts "barbell_bot: #{@barbell_bot.inspect}"
    @barbell_bot.cancel_scheduled_orders
    if @barbell_bot.update(status: 'deleted')
      redirect_to barbell_bots_path, notice: 'Bot deleted successfully'
    else
      flash.now[:alert] = @barbell_bot.errors.messages.values.join(', ')
      render :show, status: :unprocessable_entity
    end

    # result = RemoveBot.call(bot_id: params[:id], user: current_user)

    # if result.success?
    #   render json: { data: true }, status: 200
    # else
    #   render json: { id: params[:id], errors: result.errors }, status: 422
    # end
  end

  def create_api_keys
    @barbell_bot = current_user.bots.barbell.find(api_key_params[:bot_id])
    @api_key = ApiKey.new(
      user: current_user,
      exchange_id: api_key_params[:exchange_id],
      key: api_key_params[:key],
      secret: api_key_params[:secret],
      status: 'pending',
      passphrase: '',                   # TODO: required?
      german_trading_agreement: false   # TODO: required?
    )

    # TODO: verify apikey works

    if @api_key.save
      redirect_to barbell_bot_path(@barbell_bot), notice: 'API keys saved successfully'
    else
      flash.now[:alert] = @api_key.errors.messages.values.join(', ')
      render :api_keys, status: :unprocessable_entity
    end
  end

  def start
    @barbell_bot = current_user.bots.barbell.find(params[:barbell_bot_id])
    if @barbell_bot.update(status: 'pending', restarts: 0, delay: 0, current_delay: 0)
      Bot::SetBarbellOrdersJob.perform_later(@barbell_bot.id)
      redirect_to barbell_bot_path(@barbell_bot)
    else
      flash.now[:alert] = @barbell_bot.errors.messages.values.join(', ')
      render :show, status: :unprocessable_entity
    end
  end

  def stop
    @barbell_bot = current_user.bots.barbell.find(params[:barbell_bot_id])
    @barbell_bot.cancel_scheduled_orders
    if @barbell_bot.update(status: 'stopped')
      redirect_to barbell_bot_path(@barbell_bot)
    else
      flash.now[:alert] = @barbell_bot.errors.messages.values.join(', ')
      render :show, status: :unprocessable_entity
    end

    # render json: { stop: true }
    #
    #
    # result = StopBot.call(params[:id])

    # if result.success?
    #   data = present_bot(result.data)
    #   render json: { data: data }, status: 200
    # else
    #   render json: { id: params[:id], errors: result.errors }, status: 422
    # end
    #
  end

  private

  # def store_barbell_bot_params_in_session
  #   session[:barbell_bot_params] = barbell_bot_params.to_h
  # end

  def barbell_bot_params
    params.require(:bot).permit(
      :quote_amount,
      :quote,
      :interval,
      :base0,
      :base1,
      :allocation0,
      :exchange_id
    )
  end

  def api_key_params
    params.require(:api_key).permit(:key, :secret, :bot_id, :exchange_id)
  end

  def trader_key_id_for_user(exchange)
    current_user.api_keys.trading.where(exchange_id: exchange.id, status: 'correct').first&.id
  end
end
