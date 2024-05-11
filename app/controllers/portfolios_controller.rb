require 'utilities/time'

class PortfoliosController < ApplicationController
  # before_action :initialize_session, only: [:show]
  before_action :set_portfolio

  def show
    @smart_allocations = @portfolio.get_smart_allocations if @portfolio.smart_allocation_on?
    simulate_portfolio
  end

  def update_benchmark
    new_benchmark = portfolio_params[:benchmark]
    return if @portfolio.benchmark == new_benchmark

    if Portfolio.benchmarks.include?(new_benchmark) && @portfolio.update(benchmark: new_benchmark)
      simulate_portfolio
      # render :show
      respond_to do |format|
        format.html { redirect_to portfolio_analyzer_path }
        format.turbo_stream { render 'refresh_backtest_results' }
      end
    else
      redirect_to portfolio_analyzer_path, alert: 'Invalid benchmark value.'
    end
  end

  def toggle_smart_allocation
    return if @portfolio.smart_allocation_on? == portfolio_params[:smart_allocation_on]

    if @portfolio.update(smart_allocation_on: portfolio_params[:smart_allocation_on])
      if @portfolio.smart_allocation_on?
        @portfolio.set_smart_allocations!
        @smart_allocations = @portfolio.get_smart_allocations
      end
      respond_to do |format|
        format.turbo_stream { render 'refresh_allocations' }
        format.html { redirect_to portfolio_analyzer_path, notice: 'Smart allocations have been updated.' }
      end
    else
      redirect_to portfolio_analyzer_path, alert: 'Invalid smart allocation value.'
    end
  end

  def update_risk_level
    new_risk_level = Portfolio.risk_levels.key(portfolio_params[:risk_level].to_i)
    return if @portfolio.risk_level == new_risk_level

    if Portfolio.risk_levels.include?(new_risk_level) && @portfolio.update(risk_level: new_risk_level)
      @portfolio.set_smart_allocations!
      @smart_allocations = @portfolio.get_smart_allocations
      respond_to do |format|
        format.turbo_stream { render 'refresh_allocations' }
        format.html { redirect_to portfolio_analyzer_path, notice: 'Smart allocations have been updated.' }
      end
    else
      redirect_to portfolio_analyzer_path, alert: 'Invalid smart allocation value.'
    end
  end

  def normalize_allocations
    @portfolio.normalize_allocations!
    respond_to do |format|
      format.turbo_stream { render 'refresh_allocations' }
      format.html { redirect_to portfolio_analyzer_path, notice: 'Portfolio allocations have been normalized.' }
    end
  end

  def simulate
    simulate_portfolio
    render partial: 'backtest_results', locals: { portfolio: @portfolio, labels: @data_labels, series: @data_series }
  end

  private

  def portfolio_params
    params.require(:portfolio).permit(:benchmark, :strategy, :backtest_start_date, :risk_level, :smart_allocation_on)
  end

  def set_portfolio
    @portfolio = current_user.portfolios.first
    return if @portfolio.present?

    @portfolio = Portfolio.new(user: current_user)
    @portfolio.save
  end

  def all_asset_tickers
    # move to service
    source = 'binance'
    timeframe = '1d'
    expires_in = Utilities::Time.seconds_to_midnight_utc.seconds
    all_symbols = Rails.cache.fetch("symbols_#{source}_#{timeframe}", expires_in: expires_in) do
      client = FinancialDataApiClient.new
      symbols_result = client.symbols(source, timeframe)
      return {} if symbols_result.failure?

      symbols_result.data
    end
    all_symbols.map { |s| s[0...-4] }.sort!
  end

  def simulate_portfolio
    # move to service
    return if !@portfolio.normalized_allocations?

    portfolio_type = @portfolio.strategy
    assets_str = @portfolio.assets.map(&:ticker).join('_')
    allocations_str = @portfolio.assets.map(&:allocation).join('_')
    benchmark = @portfolio.benchmark
    start_date = @portfolio.backtest_start_date || '2021-01-01'
    cache_key = "simulate_#{portfolio_type}_#{assets_str}_#{allocations_str}_#{benchmark}_#{start_date}"
    expires_in = Utilities::Time.seconds_to_midnight_utc.seconds
    metrics = Rails.cache.fetch(cache_key, expires_in: expires_in) do
      client = FinancialDataApiClient.new
      symbols = @portfolio.assets.map { |a| "#{a.ticker}/USDT" }.join(',')
      metrics_result = client.metrics(symbols, allocations_str.gsub('_', ','), benchmark, start_date, portfolio_type)
      return if metrics_result.failure?

      metrics_result.data
    end

    @metrics = {
      expectedReturn: metrics['metrics']['expectedReturn'].round(2),
      volatility: metrics['metrics']['volatility'].round(2),
      alpha: metrics['metrics']['alpha'].round(2),
      beta: metrics['metrics']['beta'].round(2),
      sharpeRatio: metrics['metrics']['sharpeRatio'].round(2),
      sortinoRatio: metrics['metrics']['sortinoRatio'].round(2),
      treynorRatio: metrics['metrics']['treynorRatio'].round(2),
      rSquared: metrics['metrics']['rSquared'].round(2),
      valueAtRisk: metrics['metrics']['valueAtRisk'].round(2),
      conditionalValueAtRisk: metrics['metrics']['conditionalValueAtRisk'].round(2),
      omegaRatio: metrics['metrics']['omegaRatio'].round(2),
      calmarRatio: metrics['metrics']['calmarRatio'].round(2),
      ulcerIndex: metrics['metrics']['ulcerIndex'].round(2),
      maxDrawdown: metrics['metrics']['maxDrawdown'].round(2),
      cagr: metrics['metrics']['cagr'].round(2),
      informationRatio: metrics['metrics']['informationRatio'].round(2)
    }

    @data_labels = metrics['timeSeries']['labels']
    @data_series = metrics['timeSeries']['series']
  end
end
