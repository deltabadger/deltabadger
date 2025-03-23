class PortfolioAssetsController < ApplicationController
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

    @asset = @portfolio.portfolio_assets.new(asset_params)
    if @asset.save
      set_backtest_data
      respond_to do |format|
        format.turbo_stream
        format.html { redirect_to portfolios_path, notice: 'Asset was successfully added.' }
      end
    else
      flash.now[:alert] = @asset.errors.messages.values.join(', ')
      respond_to do |format|
        format.turbo_stream { render turbo_stream: turbo_stream_prepend_flash, status: :unprocessable_entity }
        format.html { redirect_to portfolios_path, alert: 'Invalid asset.' }
      end
    end
  end

  def destroy
    if @asset.destroy
      set_backtest_data
      respond_to do |format|
        format.turbo_stream
        format.html { redirect_to portfolios_path, notice: 'Asset was successfully removed.' }
      end
    else
      flash.now[:alert] = 'Invalid asset.'
      respond_to do |format|
        format.turbo_stream { render turbo_stream: turbo_stream_prepend_flash, status: :unprocessable_entity }
        format.html { redirect_to portfolios_path, alert: 'Invalid asset.' }
      end
    end
  end

  def update
    if @asset.update(allocation: asset_params[:allocation])
      set_backtest_data
      respond_to do |format|
        format.turbo_stream
        format.html { redirect_to portfolios_path, notice: 'Asset allocation was successfully updated.' }
      end
    else
      flash.now[:alert] = 'Invalid allocation value.'
      respond_to do |format|
        format.turbo_stream { render turbo_stream: turbo_stream_prepend_flash, status: :unprocessable_entity }
        format.html { redirect_to portfolios_path, alert: 'Invalid allocation value.' }
      end
    end
  end

  private

  def asset_params
    params.require(:portfolio_asset).permit(:ticker, :name, :allocation, :category, :color, :api_id, :url, :country, :exchange)
  end

  def set_asset
    if params[:id].blank?
      redirect_to portfolios_path, alert: 'No Asset ID provided.'
      return
    end
    @asset = @portfolio.portfolio_assets.find(params[:id])
  rescue ActiveRecord::RecordNotFound
    redirect_to portfolios_path, alert: 'Asset not found.'
  end

  def set_portfolio
    if params[:portfolio_id].blank?
      redirect_to portfolios_path, alert: 'No Portfolio ID provided.'
      return
    end
    @portfolio = current_user.portfolios.find(params[:portfolio_id])
  rescue ActiveRecord::RecordNotFound
    redirect_to portfolios_path, alert: 'Portfolio not found.'
  end

  def set_backtest_data
    return unless @portfolio.allocations_are_normalized?

    @backtest = @portfolio.backtest
    @backtest['compare_to'] = @portfolio.compare_to.map do |portfolio_id|
      portfolio = current_user.portfolios.find(portfolio_id)
      if portfolio.portfolio_assets.present? && portfolio.allocations_are_normalized?
        [portfolio.label, portfolio.backtest(custom_start_date: @portfolio.backtest_start_date)]
      end
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
