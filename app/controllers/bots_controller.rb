class BotsController < ApplicationController
  include Pagy::Backend

  before_action :authenticate_user!
  before_action :set_bot, except: %i[index create new_bot_type]

  def index
    @bots = current_user.bots.not_deleted.order(id: :desc).includes(:exchange)
  end

  def create
    @bot = current_user.bots.barbell.new(settings: { interval: Bot::INTERVALS.first, allocation0: 0.5 })
    if @bot.save
      render turbo_stream: turbo_stream_redirect(bot_path(@bot))
    else
      flash.now[:alert] = @bot.errors.messages.values.flatten.to_sentence
      render turbo_stream: turbo_stream_prepend_flash, status: :unprocessable_entity
    end
  end

  def show
    return redirect_to bots_path if @bot.deleted?

    @other_bots = current_user.bots.not_deleted.order(id: :desc).where.not(id: @bot.id)
    if @bot.legacy?
      respond_to do |format|
        format.html { render 'home/dashboard' }
        format.json { render json: @bot }
      end
    else
      @pagy, @orders = pagy_countless(@bot.transactions.order(created_at: :desc), items: 10)
    end
  end

  def edit; end

  def update
    if !@bot.legacy? && @bot.update(update_params)
      if @bot.exchange.present? && @bot.available_exchanges_for_current_settings.exclude?(@bot.exchange)
        @bot.update!(exchange_id: nil)
        flash.now[:alert] = 'Exchange not supported for current settings'
      end
      # flash.now[:notice] = t('alert.bot.bot_updated')
    elsif @bot.legacy? && @bot.update(update_params_legacy)
      # flash.now[:notice] = t('alert.bot.bot_updated')
    else
      flash.now[:alert] = @bot.errors.messages.values.flatten.to_sentence
      render turbo_stream: turbo_stream_prepend_flash, status: :unprocessable_entity
    end
  end

  def destroy
    if @bot.delete
      flash[:notice] = 'Bot deleted successfully'
      render turbo_stream: turbo_stream_redirect(bots_path)
    else
      flash.now[:alert] = @bot.errors.messages.values.flatten.to_sentence
      render turbo_stream: turbo_stream_prepend_flash, status: :unprocessable_entity
    end
  end

  def new_api_key
    @api_key = @bot.api_key
  end

  def create_api_key
    @api_key = @bot.api_key
    @api_key.key = api_key_params[:key]
    @api_key.secret = api_key_params[:secret]

    if @api_key.save
      @bot.set_exchange_client
      flash[:notice] = 'API keys saved successfully'
      render turbo_stream: turbo_stream_page_refresh
    else
      render :new_api_key, status: :unprocessable_entity
    end
  end

  def start
    return if @bot.start(ignore_missed_orders: Utilities::String.to_boolean(params[:ignore_missed_orders]))

    flash.now[:alert] = @bot.errors.messages.values.flatten.to_sentence
    render turbo_stream: turbo_stream_prepend_flash, status: :unprocessable_entity
  end

  def stop
    return if @bot.stop

    flash.now[:alert] = @bot.errors.messages.values.flatten.to_sentence
    render turbo_stream: turbo_stream_prepend_flash, status: :unprocessable_entity
  end

  def asset_search
    asset_type = params[:asset_field] == 'quote' ? :quote_asset : :base_asset
    assets = @bot.available_assets_for_current_settings(asset_type: asset_type)
    @query_assets = filter_assets_by_query(assets: assets, query: params[:query] || '')
    @asset_field = params[:asset_field]
  end

  def confirm_restart
    @bot.calculate_pending_quote_amount
  end

  def confirm_restart_legacy; end

  def new_bot_type; end

  private

  def set_bot
    @bot = current_user.bots.find(params[:id])
  rescue ActiveRecord::RecordNotFound
    redirect_to dashboard_path, alert: t('bots.not_found')
  end

  def barbell_bot_params
    params.require(:bots_barbell).permit(
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

  def basic_bot_params
    params.require(:bots_basic).permit(:label)
  end

  def withdrawal_bot_params
    params.require(:bots_withdrawal).permit(:label)
  end

  def webhook_bot_params
    params.require(:bots_webhook).permit(:label)
  end

  def api_key_params
    params.require(:api_key).permit(:key, :secret)
  end

  def update_params
    permitted_params = barbell_bot_params
    settings_params = permitted_params.except(:exchange_id, :label)

    settings_params.transform_values! { |v| Utilities::String.numeric?(v) ? v.to_f : v }

    {
      settings: @bot.settings.merge(settings_params),
      exchange_id: permitted_params[:exchange_id],
      label: permitted_params[:label]
    }.compact
  end

  def update_params_legacy
    case @bot.type
    when 'Bots::Basic'
      basic_bot_params
    when 'Bots::Withdrawal'
      withdrawal_bot_params
    when 'Bots::Webhook'
      webhook_bot_params
    end
  end

  def filter_assets_by_query(assets:, query:)
    return assets if query.blank?

    assets
      .map { |asset| [asset, similarities_for(asset, query.downcase)] }
      .select { |_, similarities| similarities.first >= 0.8 }
      .sort_by { |_, similarities| similarities.map(&:-@) }
      .map(&:first)
  end

  def similarities_for(asset, query)
    [
      JaroWinkler.similarity(asset[:ticker].downcase.to_s, query),
      JaroWinkler.similarity(asset[:name].downcase.to_s, query)
    ].sort.reverse
  end
end
