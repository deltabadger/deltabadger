class BotsController < ApplicationController
  include Pagy::Backend
  include Bots::Botable

  before_action :authenticate_user!
  before_action :set_bot, only: %i[show edit update]

  def index
    @filter = params[:filter] || 'all'
    @bots = current_user.bots.not_deleted.includes(:exchange)
    case @filter
    when 'active'
      @bots = @bots.working
    when 'inactive'
      @bots = @bots.where(status: %i[created stopped])
    end
    @bots = @bots.order(label: :asc)

    @total_bots = current_user.bots.not_deleted.size
    @has_active = current_user.bots.not_deleted.working.exists?
    @has_inactive = current_user.bots.not_deleted.where(status: %i[created stopped]).exists?
    @show_filters = @total_bots > 1 && @has_active && @has_inactive

    @pnl_hash = {}
    @loading_hash = {}
    @bots.each do |bot|
      next unless bot.dca_single_asset? || bot.dca_dual_asset?

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
      @other_bots = current_user.bots.not_deleted.order(label: :asc).where.not(id: @bot.id).pluck(:id, :label, :type)

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
      render :update, status: :unprocessable_entity
    end
  end

  private

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
    if @bot.dca_single_asset?
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
