class AssetsController < ApplicationController
  before_action :set_asset, only: %i[destroy update]

  def create
    # add condition asset ticker must be valid
    @portfolio = Portfolio.find(asset_params[:portfolio_id])
    @asset = @portfolio.assets.new(asset_params)
    if !asset_in_portfolio? && @asset.save
      query_assets_result = PortfolioAnalyzerManager::QueryAssetsGetter.call(session[:query], @portfolio)
      @query_assets = query_assets_result.failure? ? [] : query_assets_result.data
      respond_to do |format|
        format.turbo_stream
        format.html { redirect_to portfolio_analyzer_path, notice: 'Asset was successfully added.'}
      end
    else
      redirect_to portfolio_analyzer_path, alert: 'Invalid asset'
    end
  end

  def destroy
    if @asset.destroy
      query_assets_result = PortfolioAnalyzerManager::QueryAssetsGetter.call(session[:query], @asset.portfolio)
      @query_assets = query_assets_result.failure? ? [] : query_assets_result.data
      respond_to do |format|
        format.turbo_stream
        format.html { redirect_to portfolio_analyzer_path, notice: 'Asset was successfully removed.'}
      end
    else
      redirect_to portfolio_analyzer_path, alert: 'Invalid asset'
    end
  end

  def update
    if @asset.update(allocation: params[:allocation])
      respond_to do |format|
        format.turbo_stream
        format.html { redirect_to portfolio_analyzer_path, notice: 'Asset was successfully updated.'}
      end
    else
      puts 'oshit'
      redirect_to portfolio_analyzer_path, alert: 'Invalid allocation value.'
    end
  end

  private

  def asset_params
    params.require(:asset).permit(:ticker, :allocation, :portfolio_id)
  end

  def set_asset
    if params[:id].blank?
      redirect_to portfolio_analyzer_path, alert: 'No Asset ID provided.'
      return
    end
    @asset = Asset.find(params[:id])
  rescue ActiveRecord::RecordNotFound
    redirect_to portfolio_analyzer_path, alert: 'Asset not found.'
  end

  def asset_in_portfolio?
    @portfolio.assets.find_by(ticker: asset_params[:ticker]).present?
  end

  def get_current_session_query_assets
    query_asset_tickers = PortfolioAnalyzerManager::QueryAssetsTickersGetter.call(session[:query])
    return [] if query_asset_tickers.failure?

    query_asset_tickers.data.map { |a| Asset.new(ticker: a, portfolio_id: @portfolio.id) }
  end
end
