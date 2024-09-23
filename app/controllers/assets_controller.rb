class AssetsController < ApplicationController
  include ApplicationHelper

  before_action :authenticate_user!
  before_action :set_portfolio
  before_action :set_asset, only: %i[destroy update]
  before_action :set_last_assets, only: %i[create destroy]
  after_action :save_last_assets, only: %i[create destroy]

  def new
    session[:query] = params[:query]
    query_assets_result = PortfolioAnalyzerManager::QueryAssetsGetter.call(session[:query], @portfolio)
    @query_assets = query_assets_result.failure? ? [] : query_assets_result.data
  end

  def create
    # TODO: add condition asset ticker must be valid

    if @portfolio.max_assets_reached?
      flash.now[:alert] = 'Maximum number of assets reached.'
      respond_to do |format|
        format.turbo_stream { render turbo_stream: render_turbo_stream_flash_messages, status: :unprocessable_entity }
        format.html { redirect_to portfolio_analyzer_path, alert: 'Maximum number of assets reached.' }
      end
      return
    end

    @asset = @portfolio.assets.new(asset_params)
    if !asset_in_portfolio? && @asset.save
      set_backtest_data
      respond_to do |format|
        format.turbo_stream
        format.html { redirect_to portfolio_analyzer_path, notice: 'Asset was successfully added.' }
      end
    else
      flash.now[:alert] = 'Invalid asset.'
      respond_to do |format|
        format.turbo_stream { render turbo_stream: render_turbo_stream_flash_messages, status: :unprocessable_entity }
        format.html { redirect_to portfolio_analyzer_path, alert: 'Invalid asset.' }
      end
    end
  end

  def destroy
    if @asset.destroy
      set_backtest_data
      respond_to do |format|
        format.turbo_stream
        format.html { redirect_to portfolio_analyzer_path, notice: 'Asset was successfully removed.' }
      end
    else
      flash.now[:alert] = 'Invalid asset.'
      respond_to do |format|
        format.turbo_stream { render turbo_stream: render_turbo_stream_flash_messages, status: :unprocessable_entity }
        format.html { redirect_to portfolio_analyzer_path, alert: 'Invalid asset.' }
      end
    end
  end

  def update
    if @asset.update(allocation: asset_params[:allocation])
      set_backtest_data
      respond_to do |format|
        format.turbo_stream
        format.html { redirect_to portfolio_analyzer_path, notice: 'Asset allocation was successfully updated.' }
      end
    else
      flash.now[:alert] = 'Invalid allocation value.'
      respond_to do |format|
        format.turbo_stream { render turbo_stream: render_turbo_stream_flash_messages, status: :unprocessable_entity }
        format.html { redirect_to portfolio_analyzer_path, alert: 'Invalid allocation value.' }
      end
    end
  end

  private

  def asset_params
    params.require(:asset).permit(:ticker, :name, :allocation, :category, :color, :api_id, :url, :country, :exchange)
  end

  def set_asset
    if params[:id].blank?
      redirect_to portfolio_analyzer_path, alert: 'No Asset ID provided.'
      return
    end
    @asset = @portfolio.assets.find(params[:id])
  rescue ActiveRecord::RecordNotFound
    redirect_to portfolio_analyzer_path, alert: 'Asset not found.'
  end

  def set_portfolio
    if params[:portfolio_id].blank?
      redirect_to portfolio_analyzer_path, alert: 'No Portfolio ID provided.'
      return
    end
    @portfolio = current_user.portfolios.find(params[:portfolio_id])
  rescue ActiveRecord::RecordNotFound
    redirect_to portfolio_analyzer_path, alert: 'Portfolio not found.'
  end

  def asset_in_portfolio?
    @portfolio.assets.find_by(api_id: asset_params[:api_id]).present?
  end

  def set_backtest_data
    return unless @portfolio.allocations_are_normalized?

    @backtest = @portfolio.backtest
    @backtest['compare_to'] = @portfolio.compare_to.map do |portfolio_id|
      portfolio = current_user.portfolios.find(portfolio_id)
      [portfolio.label, portfolio.color, portfolio.backtest] if portfolio.assets.present? && portfolio.allocations_are_normalized?
    end.compact
  end

  def set_last_assets
    @last_active_assets_ids = session[:last_active_assets_ids] || []
    @last_idle_assets_ids = session[:last_idle_assets_ids] || []
  end

  def save_last_assets
    session[:last_active_assets_ids] = @portfolio.active_assets.map(&:id)
    session[:last_idle_assets_ids] = @portfolio.idle_assets.map(&:id)
  end
end
