class BotsController < ApplicationController
  include Pagy::Backend

  before_action :authenticate_user!
  before_action :set_new_barbell_bot,
                only: %i[barbell_new_step_to_first_asset barbell_new_step_to_second_asset barbell_new_step_exchange
                         barbell_new_step_api_key barbell_new_step_api_key_create barbell_new_step_from_asset
                         barbell_new_step_confirm]
  before_action :set_bot,
                only: %i[show edit update destroy confirm_destroy new_api_key create_api_key asset_search start
                         confirm_restart confirm_restart_legacy stop]

  def index
    return render 'bots/react_dashboard' if params[:create] # TODO: remove this once the legacy dashboard is removed

    @bots = current_user.bots.not_deleted.includes(:exchange).order(id: :desc)
  end

  def new; end

  def barbell_new_step_to_first_asset
    # FIXME: we need this sleep because this method will render a modal while being called from another modal.
    # The problem is that turbo has not yet cleaned up the modal object and it tries to render this modal
    # into the same modal partial, and crashes.
    # Seems the issue is actually related to the modal--base#animateOutCloseAndCleanUp action, which is triggered
    # but not awaited to finish before rendering the modal.
    # The FIX must address both upgrade_upgrade_instructions_path and barbell_new_step_to_first_asset_bots_path.
    a = Time.current
    assets = @bot.available_assets_for_current_settings(asset_type: :base_asset, include_exchanges: true)
    @assets = filter_assets_by_query(assets: assets, query: barbell_bot_params[:query])
    b = Time.current
    sleep [0.25 - (b - a), 0].max
    render 'bots/barbell/new/step_to_first_asset'
  end

  def barbell_new_step_to_second_asset
    assets = @bot.available_assets_for_current_settings(asset_type: :base_asset, include_exchanges: true)
    @assets = filter_assets_by_query(assets: assets, query: barbell_bot_params[:query])
    render 'bots/barbell/new/step_to_second_asset'
  end

  def barbell_new_step_exchange
    exchanges = @bot.available_exchanges_for_current_settings
    @exchanges = filter_exchanges_by_query(exchanges: exchanges, query: barbell_bot_params[:query])
    puts "exchanges: #{@exchanges.inspect}"
    render 'bots/barbell/new/step_exchange'
  end

  def barbell_new_step_api_key
    @api_key = @bot.api_key
    if @api_key.correct?
      redirect_to barbell_new_step_from_asset_bots_path(
        bots_barbell: {
          label: @bot.label,
          base0_asset_id: @bot.base0_asset_id,
          base1_asset_id: @bot.base1_asset_id,
          quote_asset_id: @bot.quote_asset_id,
          exchange_id: @bot.exchange_id
        }
      )
    else
      render 'bots/barbell/new/step_api_key'
    end
  end

  def barbell_new_step_api_key_create
    @api_key = @bot.api_key
    @api_key.key = api_key_params[:key]
    @api_key.secret = api_key_params[:secret]
    if @api_key.save
      redirect_to barbell_new_step_from_asset_bots_path(
        bots_barbell: {
          label: @bot.label,
          base0_asset_id: @bot.base0_asset_id,
          base1_asset_id: @bot.base1_asset_id,
          quote_asset_id: @bot.quote_asset_id,
          exchange_id: @bot.exchange_id
        }
      )
    else
      render 'bots/barbell/new/step_api_key', status: :unprocessable_entity
    end
  end

  def barbell_new_step_from_asset
    assets = @bot.available_assets_for_current_settings(asset_type: :quote_asset, include_exchanges: true)
    @assets = filter_assets_by_query(assets: assets, query: barbell_bot_params[:query])
    render 'bots/barbell/new/step_from_asset'
  end

  def barbell_new_step_confirm
    @bot.interval ||= Bot::INTERVALS.include?('day') ? 'day' : Bot::INTERVALS.first
    @bot.allocation0 ||= 0.5
    render 'bots/barbell/new/step_confirm'
  end

  def create
    params_to_create = barbell_bot_params_as_hash
    @bot = current_user.bots.barbell.new(params_to_create)
    if @bot.save && @bot.start(ignore_missed_orders: true)
      render turbo_stream: turbo_stream_redirect(bot_path(@bot))
    else
      # FIXME: flash messages are not shown as they are rendered behind the modal
      flash.now[:alert] = @bot.errors.messages.values.flatten.to_sentence
      render turbo_stream: turbo_stream_prepend_flash, status: :unprocessable_entity
    end
  end

  def show
    return redirect_to bots_path if @bot.deleted?

    @other_bots = current_user.bots.not_deleted.order(id: :desc).where.not(id: @bot.id)
    if @bot.legacy?
      # TODO: remove this once the legacy dashboard is removed
      respond_to do |format|
        format.html { render 'bots/react_dashboard' }
        format.json { render json: @bot }
      end
    elsif request.format.turbo_stream?
      @pagy, @orders = pagy_countless(@bot.transactions.includes(:exchange).order(created_at: :desc), items: 10)
    end
  end

  # TODO: move to custom :show logic according to bot type
  def show_index_bot
    render 'bots/index_bot'
  end

  def edit; end

  def update
    if @bot.barbell?
      barbell_bot_params_to_update = barbell_bot_params_as_hash
      barbell_bot_params_to_update[:settings] = @bot.settings.merge(barbell_bot_params_to_update[:settings])
      if @bot.update(barbell_bot_params_to_update)
        # flash.now[:notice] = t('alert.bot.bot_updated')
      else
        flash.now[:alert] = @bot.errors.messages.values.flatten.to_sentence
        render turbo_stream: turbo_stream_prepend_flash, status: :unprocessable_entity
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

    if @api_key.save
      flash[:notice] = t('errors.bots.api_key_success')
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
    asset_type = params[:asset_field] == 'quote_asset_id' ? :quote_asset : :base_asset
    assets = @bot.available_assets_for_current_settings(asset_type: asset_type, include_exchanges: true)
    @assets = filter_assets_by_query(assets: assets, query: params[:query])
    @asset_field = params[:asset_field]
  end

  def confirm_restart
    @bot.calculate_pending_quote_amount
  end

  def confirm_restart_legacy; end

  private

  def set_bot
    @bot = current_user.bots.find(params[:id])
  rescue ActiveRecord::RecordNotFound
    redirect_to bots_path, alert: t('bot.not_found')
  end

  def set_new_barbell_bot
    @bot = current_user.bots.barbell.new(barbell_bot_params_as_hash)
  end

  def barbell_bot_params
    params.require(:bots_barbell).permit(
      :quote_amount,
      :quote_asset_id,
      :interval,
      :base0_asset_id,
      :base1_asset_id,
      :allocation0,
      :market_cap_adjusted,
      :exchange_id,
      :label,
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
      pp[:base0_asset_id] = pp[:base0_asset_id].present? ? pp[:base0_asset_id].to_i : nil
      pp[:base1_asset_id] = pp[:base1_asset_id].present? ? pp[:base1_asset_id].to_i : nil
      pp[:quote_asset_id] = pp[:quote_asset_id].present? ? pp[:quote_asset_id].to_i : nil
      pp[:market_cap_adjusted] = %w[1 true].include?(pp[:market_cap_adjusted]) if pp[:market_cap_adjusted].present?
      pp[:quote_amount] = pp[:quote_amount].present? ? pp[:quote_amount].to_f : nil
      pp[:allocation0] = pp[:allocation0].present? ? pp[:allocation0].to_f : nil
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
    return assets.order(:market_cap_rank) if query.blank?

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
