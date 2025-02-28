class BarbellBotsController < ApplicationController
  include Pagy::Backend

  before_action :authenticate_user!
  before_action :set_barbell_bot,
                only: %i[show edit update destroy new_api_key create_api_key start stop asset_search]

  def index
    @bots = current_user.bots.not_deleted.barbell.order(id: :desc)
  end

  def create
    @barbell_bot = current_user.bots.barbell.new(settings: { interval: 'day', allocation0: 0.5 })
    if @barbell_bot.save
      render turbo_stream: turbo_stream_redirect(barbell_bot_path(@barbell_bot))
    else
      flash.now[:alert] = @barbell_bot.errors.messages.values.flatten.to_sentence
      render turbo_stream: turbo_stream_prepend_flash, status: :unprocessable_entity
    end
  end

  def show
    return redirect_to barbell_bots_path if @barbell_bot.deleted?

    @pagy, @transactions = pagy_countless(@barbell_bot.transactions.order(created_at: :desc), items: 10)
  end

  def edit; end

  def update
    if @barbell_bot.update(update_params)
      if @barbell_bot.exchange.present? && @barbell_bot.available_exchanges_for_current_settings.exclude?(@barbell_bot.exchange)
        @barbell_bot.update!(exchange_id: nil)
        flash.now[:alert] = 'Exchange not supported for current settings'
      end
      # flash.now[:notice] = t('alert.bot.bot_updated')
    else
      flash.now[:alert] = @barbell_bot.errors.messages.values.flatten.to_sentence
      render turbo_stream: turbo_stream_prepend_flash, status: :unprocessable_entity
    end
  end

  def destroy
    if @barbell_bot.delete
      flash[:notice] = 'Bot deleted successfully'
      render turbo_stream: turbo_stream_redirect(barbell_bots_path)
    else
      flash.now[:alert] = @barbell_bot.errors.messages.values.flatten.to_sentence
      render turbo_stream: turbo_stream_prepend_flash, status: :unprocessable_entity
    end
  end

  def new_api_key
    @api_key = @barbell_bot.api_key
  end

  def create_api_key
    @api_key = @barbell_bot.api_key
    @api_key.key = api_key_params[:key]
    @api_key.secret = api_key_params[:secret]

    if @api_key.save
      @barbell_bot.set_exchange_client
      flash[:notice] = 'API keys saved successfully'
      render turbo_stream: turbo_stream_page_refresh
    else
      render :new_api_key, status: :unprocessable_entity
    end
  end

  def start
    return if @barbell_bot.start

    flash.now[:alert] = @barbell_bot.errors.messages.values.flatten.to_sentence
    render status: :unprocessable_entity
  end

  def stop
    return if @barbell_bot.stop

    flash.now[:alert] = @barbell_bot.errors.messages.values.flatten.to_sentence
    render status: :unprocessable_entity
  end

  def asset_search
    asset_type = params[:asset_field] == 'quote' ? :quote_asset : :base_asset
    assets = @barbell_bot.available_assets_for_current_settings(asset_type: asset_type)
    @query_assets = filter_assets_by_query(assets: assets, query: params[:query] || '')
    @asset_field = params[:asset_field]
  end

  private

  def set_barbell_bot
    @barbell_bot = current_user.bots.barbell.find(params[:id])
  end

  def barbell_bot_params
    params.require(:barbell_bot).permit(
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
    params.require(:api_key).permit(:key, :secret)
  end

  def update_params
    permitted_params = barbell_bot_params
    settings_params = permitted_params.except(:exchange_id, :label)

    settings_params.transform_values! { |v| Utilities::String.numeric?(v) ? v.to_f : v }

    {
      settings: @barbell_bot.settings.merge(settings_params),
      exchange_id: permitted_params[:exchange_id],
      label: permitted_params[:label]
    }.compact
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
