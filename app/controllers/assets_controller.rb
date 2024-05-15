class AssetsController < ApplicationController
  before_action :set_asset, only: %i[destroy update]
  before_action :set_portfolio, only: %i[new create destroy]

  def new
    session[:query] = params[:query]
    query_assets_result = PortfolioAnalyzerManager::QueryAssetsGetter.call(session[:query], @portfolio)
    @query_assets = query_assets_result.failure? ? [] : query_assets_result.data
  end

  def create
    # add condition asset ticker must be valid
    @asset = @portfolio.assets.new(asset_params)
    if !asset_in_portfolio? && @asset.save
      respond_to do |format|
        format.turbo_stream
        format.html { redirect_to portfolio_analyzer_path, notice: 'Asset was successfully added.' }
      end
    else
      redirect_to portfolio_analyzer_path, alert: 'Invalid asset'
    end
  end

  def destroy
    if @asset.destroy
      respond_to do |format|
        format.turbo_stream
        format.html { redirect_to portfolio_analyzer_path, notice: 'Asset was successfully removed.' }
      end
    else
      redirect_to portfolio_analyzer_path, alert: 'Invalid asset'
    end
  end

  def update
    if @asset.update(allocation: asset_params[:allocation])
      render partial: 'portfolios/normalize', locals: { portfolio: @asset.portfolio }
    else
      redirect_to portfolio_analyzer_path, alert: 'Invalid allocation value.'
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
    @asset = Asset.find(params[:id])
  rescue ActiveRecord::RecordNotFound
    redirect_to portfolio_analyzer_path, alert: 'Asset not found.'
  end

  def set_portfolio
    if params[:portfolio_id].blank?
      redirect_to portfolio_analyzer_path, alert: 'No Portfolio ID provided.'
      return
    end
    @portfolio = Portfolio.find(params[:portfolio_id])
  rescue ActiveRecord::RecordNotFound
    redirect_to portfolio_analyzer_path, alert: 'Portfolio not found.'
  end

  def asset_in_portfolio?
    @portfolio.assets.find_by(ticker: asset_params[:ticker], category: asset_params[:category]).present?
  end
end
