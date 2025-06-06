class BotsController < ApplicationController
  include Pagy::Backend
  include Bots::Botable

  before_action :authenticate_user!
  before_action :set_bot, only: %i[show edit update]

  def index
    return render 'bots/react_dashboard' if params[:create] # TODO: remove this once the legacy dashboard is removed

    @bots = current_user.bots.not_deleted.includes(:exchange).order(id: :desc)
    @pnl_hash = {}
    @loading_hash = {}
    @bots.each do |bot|
      next unless bot.dca_single_asset? || bot.dca_dual_asset? || bot.basic?

      metrics_with_current_prices = bot.metrics_with_current_prices_from_cache
      @pnl_hash[bot.id] = metrics_with_current_prices[:pnl] unless metrics_with_current_prices.nil?
      @loading_hash[bot.id] = metrics_with_current_prices.nil?
    end
  end

  def new
    # make every every new bot config start fresh
    session[:bot_config] = {}
  end

  def show
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
        if @bot.dca_single_asset?
          @decimals = {
            @bot.base_asset.symbol => @bot.decimals[:base],
            @bot.quote_asset.symbol => @bot.decimals[:quote]
          }
        elsif @bot.dca_dual_asset?
          @decimals = {
            @bot.base0_asset.symbol => @bot.decimals[:base0],
            @bot.base1_asset.symbol => @bot.decimals[:base1],
            @bot.quote_asset.symbol => @bot.decimals[:quote]
          }
        end

        metrics_with_current_prices_and_candles = @bot.metrics_with_current_prices_and_candles_from_cache
        @loading = metrics_with_current_prices_and_candles.nil?
        @metrics = @loading ? @bot.metrics : metrics_with_current_prices_and_candles
      end
    end
  end

  # TODO: move to custom :show logic according to bot type
  def show_index_bot
    render 'bots/index_bot'
  end

  def edit; end

  def update
    @bot.set_missed_quote_amount if @bot.dca_single_asset? || @bot.dca_dual_asset?

    if @bot.update(update_params)
      # flash.now[:notice] = t('alert.bot.bot_updated')
    else
      flash.now[:alert] = @bot.errors.messages.values.flatten.to_sentence
      if @bot.legacy?
        render turbo_stream: turbo_stream_prepend_flash, status: :unprocessable_entity
      else
        render :update, status: :unprocessable_entity
      end
    end
  end

  private

  def basic_bot_params
    params.require(:bots_basic).permit(:label)
  end

  def withdrawal_bot_params
    params.require(:bots_withdrawal).permit(:label)
  end

  def webhook_bot_params
    params.require(:bots_webhook).permit(:label)
  end

  def dca_single_asset_bot_params
    params.require(:bots_dca_single_asset).permit(
      :label,
      :exchange_id,
      *Bots::DcaSingleAsset.stored_attributes[:settings]
    )
  end

  def dca_dual_asset_bot_params
    params.require(:bots_dca_dual_asset).permit(
      :label,
      :exchange_id,
      *Bots::DcaDualAsset.stored_attributes[:settings]
    )
  end

  def update_params
    if @bot.basic?
      basic_bot_params
    elsif @bot.withdrawal?
      withdrawal_bot_params
    elsif @bot.webhook?
      webhook_bot_params
    elsif @bot.dca_single_asset?
      {
        settings: @bot.settings.merge(
          @bot.parse_params(dca_single_asset_bot_params).stringify_keys
        ),
        exchange_id: dca_single_asset_bot_params[:exchange_id],
        label: dca_single_asset_bot_params[:label].presence
      }.compact
    elsif @bot.dca_dual_asset?
      {
        settings: @bot.settings.merge(
          @bot.parse_params(dca_dual_asset_bot_params).stringify_keys
        ),
        exchange_id: dca_dual_asset_bot_params[:exchange_id],
        label: dca_dual_asset_bot_params[:label].presence
      }.compact
    else
      raise "Unknown bot type: #{@bot.type}"
    end
  end
end
