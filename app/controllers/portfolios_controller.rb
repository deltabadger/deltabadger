require 'utilities/time'

class PortfoliosController < ApplicationController
  # before_action :initialize_session, only: [:show]
  before_action :set_portfolio, only: %i[show toggle_smart_allocations normalize_allocations simulate]

  KNOWN_BENCHMARKS = {
    '^GSPC': 'S&P 500 Index',
    '^DJI': 'Dow Jones Industrial Average',
    '^IXIC': 'Nasdaq Composite Index',
    '^RUT': 'Russell 2000 Index'
  }.freeze
  KNOWN_STRATEGIES = ['fixed'].freeze

  def show
    simulate_portfolio
  end

  def toggle_smart_allocations
    smart_allocation_on = params[:smart_allocations_enabled] == '1'
    return if @portfolio.smart_allocation_on? == smart_allocation_on

    if @portfolio.update(smart_allocation_on: smart_allocation_on)
      @portfolio.set_smart_allocations! if smart_allocation_on
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
