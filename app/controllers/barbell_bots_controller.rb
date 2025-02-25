class BarbellBotsController < ApplicationController
  include Pagy::Backend

  before_action :authenticate_user!

  def index
    @bots = current_user.bots.not_deleted.barbell.order(id: :desc)
  end

  def new
    # quick workaround
    barbell_bot = current_user.bots.barbell.new({
                                                  settings: {
                                                    quote_amount: 0.05,
                                                    quote: 'EUR',
                                                    base0: 'BTC',
                                                    base1: 'ETH',
                                                    interval: 'hour',
                                                    allocation0: 0.5
                                                  },
                                                  exchange: Exchange.find_by(name: 'Coinbase')
                                                })
    barbell_bot.save!
    redirect_to barbell_bot_path(barbell_bot), notice: 'Bot created successfully'
    return

    new_params = {
      settings: {
        quote_amount: params.dig(:bot, :quote_amount),
        quote: params.dig(:bot, :quote),
        base0: params.dig(:bot, :base0),
        base1: params.dig(:bot, :base1),
        interval: params.dig(:bot, :interval),
        allocation0: params.dig(:bot, :allocation0)
      }.compact,
      exchange_id: params.dig(:bot, :exchange_id),
      label: params.dig(:bot, :label)
    }.compact

    @barbell_bot = current_user.bots.barbell.new(new_params)

    if %w[quote base0 base1].include?(params[:asset_type])
      assets = get_all_available_assets(asset_type: params[:asset_type].delete('01').to_sym)
      render partial: 'barbell_bots/search_asset', locals: { bot: @barbell_bot, assets: assets, asset_type: params[:asset_type] }
      return
    end

    # old implementation
    @exchanges = Exchange.available_for_barbell_bots
    @quote_assets = get_all_available_assets(asset_type: :quote_asset)
    @base_assets = get_all_available_assets(asset_type: :base_asset)
  end

  def create
    create_params = {
      settings: barbell_bot_params.except(:exchange_id, :label),
      exchange_id: barbell_bot_params[:exchange_id],
      label: barbell_bot_params[:label]
    }.compact
    @barbell_bot = current_user.bots.barbell.new(create_params)

    if @barbell_bot.save
      redirect_to barbell_bot_path(@barbell_bot), notice: 'Bot created successfully'
    else
      flash.now[:alert] = @barbell_bot.errors.messages.values.join(', ')
      render :new, status: :unprocessable_entity
    end
  end

  def show
    @barbell_bot = current_user.bots.barbell.find(params[:id])
    @pagy, @transactions = pagy_countless(Transaction.for_bot(@barbell_bot), items: 10)
    set_api_key_instance_variable_for_bot(@barbell_bot)
  end

  def edit
    @barbell_bot = current_user.bots.barbell.find(params[:id])
  end

  def update
    @barbell_bot = current_user.bots.barbell.find(params[:id])
    update_params = {
      settings: @barbell_bot.settings.merge(barbell_bot_params.except(:exchange_id, :label)),
      exchange_id: barbell_bot_params[:exchange_id],
      label: barbell_bot_params[:label]
    }.compact

    if @barbell_bot.update(update_params)
      flash.now[:notice] = t('alert.bot.bot_updated')
      respond_to do |format| # rubocop:disable Style/SymbolProc
        format.turbo_stream
      end
    else
      flash[:alert] = @barbell_bot.errors.messages.to_sentence
      render turbo_stream: turbo_stream_page_refresh, status: :unprocessable_entity
    end
  end

  def destroy
    @barbell_bot = current_user.bots.barbell.find(params[:barbell_bot_id])
    if @barbell_bot.delete
      redirect_to barbell_bots_path, notice: 'Bot deleted successfully'
    else
      flash.now[:alert] = @barbell_bot.errors.messages.to_sentence
      render turbo_stream: turbo_stream_prepend_flash, status: :unprocessable_entity
    end
  end

  def new_api_key
    @barbell_bot = current_user.bots.barbell.find(params[:id])
    set_api_key_instance_variable_for_bot(@barbell_bot)
  end

  def create_api_key
    @barbell_bot = current_user.bots.barbell.find(api_key_params[:bot_id])
    set_api_key_instance_variable_for_bot(@barbell_bot)
    @api_key.key = api_key_params[:key]
    @api_key.secret = api_key_params[:secret]

    if @api_key.save
      flash[:notice] = 'API keys saved successfully'
      render turbo_stream: turbo_stream_page_refresh
    else
      render :new_api_key, status: :unprocessable_entity
    end
  end

  def start
    @barbell_bot = current_user.bots.barbell.find(params[:id])
    if @barbell_bot.start
      respond_to do |format| # rubocop:disable Style/SymbolProc
        format.turbo_stream
      end
    else
      flash[:alert] = @barbell_bot.errors.messages.values.join(', ')
      render turbo_stream: turbo_stream_page_refresh, status: :unprocessable_entity
    end
  end

  def stop
    @barbell_bot = current_user.bots.barbell.find(params[:id])
    if @barbell_bot.stop
      respond_to do |format| # rubocop:disable Style/SymbolProc
        format.turbo_stream
      end
    else
      flash[:alert] = @barbell_bot.errors.messages.values.join(', ')
      render turbo_stream: turbo_stream_page_refresh, status: :unprocessable_entity
    end
  end

  private

  def barbell_bot_params
    params.require(:bot).permit(
      :quote_amount,
      :quote,
      :interval,
      :base0,
      :base1,
      :allocation0,
      :exchange_id,
      :label
    )
  end

  def api_key_params
    params.require(:api_key).permit(:key, :secret, :bot_id, :exchange_id)
  end

  def set_api_key_instance_variable_for_bot(bot)
    @api_key = api_key_for_bot(bot) || current_user.api_keys.new(
      exchange: bot.exchange,
      status: 'pending',
      passphrase: '',                   # TODO: required?
      german_trading_agreement: false   # TODO: required?
    )
  end

  def api_key_for_bot(bot)
    current_user.api_keys.trading.where(exchange_id: bot.exchange.id).first
  end

  def get_all_available_assets(asset_type:)
    Exchange.available_for_barbell_bots.each_with_object({}) do |exchange, assets|
      result = exchange.get_info
      next if result.failure?

      result.data[:symbols].each do |symbol|
        assets[symbol[asset_type]] ||= []
        next if assets[symbol[asset_type]].include?(exchange.name)

        assets[symbol[asset_type]] << exchange.name
      end
    end
  end
end
