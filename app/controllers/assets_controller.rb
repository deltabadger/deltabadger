class AssetsController < ApplicationController
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
    @asset = @portfolio.assets.new(asset_params)
    if !asset_in_portfolio? && @asset.save
      set_backtest_data
      respond_to do |format|
        format.turbo_stream
        format.html { redirect_to portfolio_analyzer_path, notice: 'Asset was successfully added.' }
      end
    else
      redirect_to portfolio_analyzer_path, alert: 'Invalid asset', status: :unprocessable_entity
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
      redirect_to portfolio_analyzer_path, alert: 'Invalid asset', status: :unprocessable_entity
    end
  end

  def update
    if @asset.update(allocation: asset_params[:allocation])
      @backtest = @portfolio.backtest if @portfolio.allocations_are_normalized?
      respond_to do |format|
        format.turbo_stream
        format.html { redirect_to portfolio_analyzer_path, notice: 'Asset allocation was successfully updated.' }
      end
    else
      redirect_to portfolio_analyzer_path, alert: 'Invalid allocation value.', status: :unprocessable_entity
    end
  end

  private

  def asset_params
    params.require(:asset).permit(:ticker, :name, :allocation, :category, :color)
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
    @portfolio.assets.find_by(ticker: asset_params[:ticker], category: asset_params[:category]).present?
  end

  def set_backtest_data
    @portfolio.set_smart_allocations! if @portfolio.smart_allocation_on?
    @backtest = @portfolio.backtest if @portfolio.allocations_are_normalized?
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
