class BotsController < ApplicationController
  include Pagy::Backend

  before_action :authenticate_user!
  before_action :set_new_barbell_bot,
                only: %i[barbell_new_step_to_first_asset
                         barbell_new_step_to_second_asset
                         barbell_new_step_exchange
                         barbell_new_step_api_key
                         barbell_new_step_api_key_create
                         barbell_new_step_from_asset
                         barbell_new_step_confirm
                         create]
  before_action :set_bot,
                only: %i[show
                         edit
                         update
                         destroy
                         confirm_destroy
                         new_api_key
                         create_api_key
                         asset_search
                         start
                         confirm_restart
                         confirm_restart_legacy
                         stop]

  def index
    return render 'bots/react_dashboard' if params[:create] # TODO: remove this once the legacy dashboard is removed

    @bots = current_user.bots.not_deleted.includes(:exchange).order(id: :desc)
    @pnl_hash = {}
    @loading_hash = {}
    @bots.each do |bot|
      next if bot.withdrawal? || bot.webhook?

      metrics_with_current_prices = bot.metrics_with_current_prices_from_cache
      @pnl_hash[bot.id] = metrics_with_current_prices[:pnl] unless metrics_with_current_prices.nil?
      @loading_hash[bot.id] = metrics_with_current_prices.nil?
    end
  end

  def new
    session[:barbell_bot_params] = {}
  end

  def barbell_new_step_to_first_asset
    # TODO: move this block to a better place
    @bot.base0_asset_id = nil
    available_assets = @bot.available_assets_for_current_settings(asset_type: :base_asset)
    filtered_assets = filter_assets_by_query(assets: available_assets, query: barbell_bot_params[:query])
                      .pluck(:id, :symbol, :name)
    exchanges_data = Exchange.all.pluck(:id, :name_id, :name).each_with_object([]) do |(id, name_id, name), list|
      assets = Exchange.find(id).assets.pluck(:id)
      list << [name_id, name, assets] if assets.any?
    end
    @assets = filtered_assets.map do |id, symbol, name|
      exchanges = exchanges_data.select { |_, _, assets| assets.include?(id) }
      [id, symbol, name, exchanges.map { |e_name_id, e_name, _| [e_name_id, e_name] }]
    end
    render 'bots/barbell/new/step_to_first_asset'
  end

  def barbell_new_step_to_second_asset
    @bot.base1_asset_id = nil
    available_assets = @bot.available_assets_for_current_settings(asset_type: :base_asset)
    filtered_assets = filter_assets_by_query(assets: available_assets, query: barbell_bot_params[:query])
                      .pluck(:id, :symbol, :name)
    exchanges_data = Exchange.all.pluck(:id, :name_id, :name).each_with_object([]) do |(id, name_id, name), list|
      assets = Exchange.find(id).assets.pluck(:id)
      list << [name_id, name, assets] if assets.any?
    end
    @assets = filtered_assets.map do |id, symbol, name|
      exchanges = exchanges_data.select { |_, _, assets| assets.include?(id) }
      [id, symbol, name, exchanges.map { |e_name_id, e_name, _| [e_name_id, e_name] }]
    end
    render 'bots/barbell/new/step_to_second_asset'
  end

  def barbell_new_step_exchange
    @bot.exchange_id = nil
    exchanges = @bot.available_exchanges_for_current_settings
    @exchanges = filter_exchanges_by_query(exchanges: exchanges, query: barbell_bot_params[:query])
    render 'bots/barbell/new/step_exchange'
  end

  def barbell_new_step_api_key
    @api_key = @bot.api_key
    @api_key.validate_key_permissions if @api_key.key.present? && @api_key.secret.present?
    if @api_key.correct?
      redirect_to barbell_new_step_from_asset_bots_path
    else
      render 'bots/barbell/new/step_api_key'
    end
  end

  def barbell_new_step_api_key_create
    @api_key = @bot.api_key
    @api_key.key = api_key_params[:key]
    @api_key.secret = api_key_params[:secret]
    @api_key.validate_key_permissions
    if @api_key.correct? && @api_key.save
      redirect_to barbell_new_step_from_asset_bots_path
    else
      render 'bots/barbell/new/step_api_key', status: :unprocessable_entity
    end
  end

  def barbell_new_step_from_asset
    @bot.quote_asset_id = nil
    available_assets = @bot.available_assets_for_current_settings(asset_type: :quote_asset)
    filtered_assets = filter_assets_by_query(assets: available_assets, query: barbell_bot_params[:query])
                      .pluck(:id, :symbol, :name)
    exchanges_data = Exchange.all.pluck(:id, :name_id, :name).each_with_object([]) do |(id, name_id, name), list|
      assets = Exchange.find(id).assets.pluck(:id)
      list << [name_id, name, assets] if assets.any?
    end
    @assets = filtered_assets.map do |id, symbol, name|
      exchanges = exchanges_data.select { |_, _, assets| assets.include?(id) }
      [id, symbol, name, exchanges.map { |e_name_id, e_name, _| [e_name_id, e_name] }]
    end
    render 'bots/barbell/new/step_from_asset'
  end

  def barbell_new_step_confirm
    @bot.interval ||= 'day'
    @bot.allocation0 ||= 0.5
    if @bot.quote_amount.blank? || @bot.valid?
      render 'bots/barbell/new/step_confirm'
    else
      flash.now[:alert] = @bot.errors.messages.values.flatten.to_sentence
      render 'bots/barbell/new/step_confirm', status: :unprocessable_entity
    end
  end

  def create
    if @bot.save && @bot.start(start_fresh: true)
      session[:barbell_bot_params] = nil
      render turbo_stream: turbo_stream_redirect(bot_path(@bot))
    else
      # FIXME: flash messages are not shown as they are rendered behind the modal
      flash.now[:alert] = @bot.errors.messages.values.flatten.to_sentence
      render turbo_stream: turbo_stream_prepend_flash, status: :unprocessable_entity
    end
  end

  def show
    return redirect_to bots_path if @bot.deleted?

    if request.format.turbo_stream?
      @pagy, @orders = pagy_countless(@bot.transactions.order(created_at: :desc), items: 10)
      permitted_params = params.require(:decimals).permit(*Asset.all.pluck(:symbol))
      @decimals = permitted_params.transform_values(&:to_i)
    else
      @other_bots = current_user.bots.not_deleted.order(id: :desc).where.not(id: @bot.id).pluck(:id, :label, :type)

      if @bot.legacy?
        # TODO: remove this once the legacy dashboard is removed
        respond_to do |format|
          format.html { render 'bots/react_dashboard' }
          format.json { render json: @bot }
        end
      else
        # TODO: When transactions point to real asset ids, we can use the asset ids directly instead of symbols
        @decimals = {
          @bot.base0_asset.symbol => @bot.decimals[:base0],
          @bot.base1_asset.symbol => @bot.decimals[:base1],
          @bot.quote_asset.symbol => @bot.decimals[:quote]
        }
        metrics_with_current_prices = @bot.metrics_with_current_prices_from_cache
        @loading = metrics_with_current_prices.nil?
        @metrics = @loading ? @bot.metrics : metrics_with_current_prices
      end
    end
  end

  # TODO: move to custom :show logic according to bot type
  def show_index_bot
    render 'bots/index_bot'
  end

  def edit; end

  def update
    if @bot.barbell?
      @bot.set_missed_quote_amount
      barbell_bot_params_to_update = barbell_bot_params_as_hash
      barbell_bot_params_to_update[:settings] = @bot.settings.merge(barbell_bot_params_to_update[:settings].stringify_keys)
      if @bot.update(barbell_bot_params_to_update)
        # flash.now[:notice] = t('alert.bot.bot_updated')
      else
        flash.now[:alert] = @bot.errors.messages.values.flatten.to_sentence
        render :update, status: :unprocessable_entity
      end
    elsif @bot.legacy?
      if @bot.update(update_params_legacy)
        # flash.now[:notice] = t('alert.bot.bot_updated')
      else
        flash.now[:alert] = @bot.errors.messages.values.flatten.to_sentence
        render turbo_stream: turbo_stream_prepend_flash, status: :unprocessable_entity
      end
    else
      raise "Unknown bot type: #{@bot.type}"
    end
  end

  def confirm_destroy; end

  def destroy
    if @bot.destroy
      flash[:notice] = t('errors.bots.destroy_success', bot_label: @bot.label)
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
    @api_key.validate_key_permissions
    if @api_key.correct? && @api_key.save
      flash[:notice] = t('errors.bots.api_key_success')
      render turbo_stream: turbo_stream_page_refresh
    else
      render :new_api_key, status: :unprocessable_entity
    end
  end

  def start
    return if @bot.start(start_fresh: Utilities::String.to_boolean(params[:start_fresh]))

    flash.now[:alert] = @bot.errors.messages.values.flatten.to_sentence
    render turbo_stream: turbo_stream_prepend_flash, status: :unprocessable_entity
  end

  def stop
    return if @bot.stop

    flash.now[:alert] = @bot.errors.messages.values.flatten.to_sentence
    render turbo_stream: turbo_stream_prepend_flash, status: :unprocessable_entity
  end

  def asset_search
    asset_type = params[:asset_field] == 'quote_asset_id' ? :quote_asset : :base_asset
    available_assets = @bot.available_assets_for_current_settings(asset_type: asset_type)
    filtered_assets = filter_assets_by_query(assets: available_assets, query: params[:query])
                      .pluck(:id, :symbol, :name)
    exchanges_data = Exchange.all.pluck(:id, :name_id,
                                        :name).each_with_object([]) do |(id, name_id, name), list|
      assets = Exchange.find(id).assets.pluck(:id)
      list << [name_id, name, assets] if assets.any?
    end
    @assets = filtered_assets.map do |id, symbol, name|
      exchanges = exchanges_data.select { |_, _, assets| assets.include?(id) }
      [id, symbol, name, exchanges.map { |e_name_id, e_name, _| [e_name_id, e_name] }]
    end
    @asset_field = params[:asset_field]
  end

  def confirm_restart; end

  def confirm_restart_legacy; end

  private

  def set_bot
    @bot = current_user.bots.find(params[:id])
  rescue ActiveRecord::RecordNotFound
    redirect_to bots_path, alert: t('bot.not_found')
  end

  def set_new_barbell_bot
    session[:barbell_bot_params] ||= {}
    session[:barbell_bot_params] = session[:barbell_bot_params].deep_merge(barbell_bot_params_as_hash.deep_stringify_keys)
    @bot = current_user.bots.barbell.new(session[:barbell_bot_params])
    session[:barbell_bot_params]['label'] = @bot.label
  end

  def barbell_bot_params
    params.fetch(:bots_barbell, {}).permit(
      :quote_amount,
      :quote_asset_id,
      :interval,
      :base0_asset_id,
      :base1_asset_id,
      :allocation0,
      :marketcap_allocated,
      :exchange_id,
      :label,
      :quote_amount_limited,
      :quote_amount_limit,
      :price_limited,
      :price_limit,
      :price_limit_timing_condition,
      :price_limit_price_condition,
      :price_limit_in_ticker_id,
      :smart_intervaled,
      :smart_interval_quote_amount,
      :query
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

  def barbell_bot_params_as_hash
    permitted_params = barbell_bot_params.to_h
    settings = permitted_params.except(:exchange_id, :label, :query).tap do |pp|
      pp[:base0_asset_id] = pp[:base0_asset_id].presence&.to_i
      pp[:base1_asset_id] = pp[:base1_asset_id].presence&.to_i
      pp[:quote_asset_id] = pp[:quote_asset_id].presence&.to_i
      pp[:quote_amount] = pp[:quote_amount].presence&.to_f
      pp[:allocation0] = pp[:allocation0].presence&.to_f
      pp[:marketcap_allocated] = pp[:marketcap_allocated].presence&.in?(%w[1 true])
      pp[:quote_amount_limited] = pp[:quote_amount_limited].presence&.in?(%w[1 true])
      pp[:quote_amount_limit] = pp[:quote_amount_limit].presence&.to_f
      pp[:price_limited] = pp[:price_limited].presence&.in?(%w[1 true])
      pp[:price_limit] = pp[:price_limit].presence&.to_f
      pp[:price_limit_timing_condition] = pp[:price_limit_timing_condition].presence
      pp[:price_limit_price_condition] = pp[:price_limit_price_condition].presence
      pp[:price_limit_in_ticker_id] = pp[:price_limit_in_ticker_id].presence&.to_i
      pp[:smart_intervaled] = pp[:smart_intervaled].presence&.in?(%w[1 true])
      pp[:smart_interval_quote_amount] = pp[:smart_interval_quote_amount].presence&.to_f
    end.compact

    {
      settings: settings,
      exchange_id: permitted_params[:exchange_id],
      label: permitted_params[:label].presence
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
    return assets.order(:market_cap_rank, :symbol) if query.blank?

    assets
      .map { |asset| [asset, similarities_for_asset(asset, query.downcase)] }
      .select { |_, similarities| similarities.first >= 0.7 }
      .sort_by { |asset, similarities| [similarities.map(&:-@), asset.market_cap_rank || Float::INFINITY] }
      .map(&:first)
  end

  def similarities_for_asset(asset, query)
    [
      asset.symbol.present? ? JaroWinkler.similarity(asset.symbol.downcase.to_s, query) : 0,
      asset.name.present? ? JaroWinkler.similarity(asset.name.downcase.to_s, query) : 0
    ].sort.reverse
  end

  def filter_exchanges_by_query(exchanges:, query:)
    return exchanges.order(:name) if query.blank?

    exchanges
      .map { |exchange| [exchange, similarities_for_exchange(exchange, query.downcase)] }
      .select { |_, similarities| similarities.first >= 0.7 }
      .sort_by { |_, similarities| similarities.map(&:-@) }
      .map(&:first)
  end

  def similarities_for_exchange(exchange, query)
    [
      exchange.name.present? ? JaroWinkler.similarity(exchange.name.downcase.to_s, query) : 0
    ].sort.reverse
  end
end
