require 'utilities/time'

class PortfolioAnalyzersController < ApplicationController
  before_action :initialize_session
  before_action :set_selected_assets,
                only: %i[show add_asset remove_asset update_allocation normalize_allocations smart_allocations]
  before_action :validate_allocation, only: [:update_allocation]
  before_action :validate_selected_asset, only: %i[remove_asset update_allocation]
  before_action :validate_unselected_asset, only: %i[add_asset]

  AVAILABLE_BENCHMARKS = {
    '^GSPC': 'S&P 500 Index',
    '^DJI': 'Dow Jones Industrial Average',
    '^IXIC': 'Nasdaq Composite Index',
    '^RUT': 'Russell 2000 Index'
  }.freeze
  AVAILABLE_PORTFOLIO_TYPES = ['fixed'].freeze

  def show
    simulate_current_session
    session[:query] = params[:query]
    @query_assets = get_query_assets(session[:query])
    @smart_allocations_enabled = session[:smart_allocations_enabled]

    if turbo_frame_request?
      render partial: 'asset_selector', locals: { query_assets: @query_assets }
    else
      render :show
    end
  end

  def add_asset
    @allocation = 0 # Default allocation to 0
    session[:selected_assets][@asset] = @allocation
    @query_assets = get_query_assets(session[:query])
    @smart_allocations_enabled = false

    respond_to do |format|
      format.turbo_stream
      format.html { redirect_to portfolio_analyzer_path }
    end
  end

  def remove_asset
    session[:selected_assets].delete(@asset)
    @query_assets = get_query_assets(session[:query])
    @smart_allocations_enabled = false

    respond_to do |format|
      format.turbo_stream
      format.html { redirect_to portfolio_analyzer_path }
    end
  end

  def update_allocation
    session[:selected_assets][@asset] = @allocation

    respond_to do |format|
      format.turbo_stream
      format.html { redirect_to portfolio_analyzer_path }
    end
  end

  def normalize_allocations
    total_allocation = session[:selected_assets].values.sum
    if total_allocation.zero?
      session[:selected_assets].transform_values! { |_| (1.to_f / session[:selected_assets].length).round(4) }
    else
      session[:selected_assets].transform_values! { |v| (v.to_f / total_allocation).round(4) }
    end

    # Adjust the last value to ensure the sum is exactly 1
    correction = (1.0 - session[:selected_assets].values.sum).round(4)
    session[:selected_assets].each_key do |key|
      if session[:selected_assets][key] + correction >= 0 && session[:selected_assets][key] + correction <= 1
        session[:selected_assets][key] += correction
        break
      end
    end

    respond_to do |format|
      format.turbo_stream
      format.html { redirect_to portfolio_analyzer_path }
    end
  end

  def simulate
    simulate_current_session

    respond_to do |format|
      format.turbo_stream
      format.html { redirect_to portfolio_analyzer_path }
    end
  end

  def smart_allocations
    session[:smart_allocations_enabled] = params[:smart_allocations_enabled] == '1'
    get_smart_allocations if session[:smart_allocations_enabled]
    @smart_allocations_enabled = session[:smart_allocations_enabled]

    respond_to do |format|
      format.turbo_stream { render :normalize_allocations }
      format.html { redirect_to portfolio_analyzer_path }
    end
  end

  private

  def initialize_session
    defaults = {
      selected_assets: {},
      smart_allocations_enabled: false,
      smart_allocation_selected: 2,
      benchmark: AVAILABLE_BENCHMARKS.keys.first,
      backtest_start_date: '2021-01-01',
      portfolio_type: AVAILABLE_PORTFOLIO_TYPES.first,
    }

    defaults.each do |key, value|
      session[key] ||= value
    end
  end

  def set_selected_assets
    @selected_assets = session[:selected_assets]
  end

  def validate_selected_asset
    if session[:selected_assets].key?(params[:asset])
      @asset = params[:asset]
    else
      flash[:alert] = 'Invalid asset'
      redirect_to portfolio_analyzer_path and return
    end
  end

  def validate_unselected_asset
    if !session[:selected_assets].key?(params[:asset]) && all_assets.include?(params[:asset])
      @asset = params[:asset]
    else
      flash[:alert] = 'Invalid asset'
      redirect_to portfolio_analyzer_path and return
    end
  end

  def validate_allocation
    @allocation = params[:allocation].to_f
    raise ArgumentError, 'Invalid allocation' if @allocation.negative? || @allocation > 1
  rescue ArgumentError, TypeError
    flash[:alert] = 'Invalid allocation'
    redirect_to portfolio_analyzer_path and return
  end

  def all_assets
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

  def get_query_assets(query)
    return [] if query.blank?

    available_assets = all_assets - session[:selected_assets].keys
    available_assets.filter { |a| a.include?(query.upcase) }
  end

  def get_smart_allocations
    # move to service
    portfolio_type = session[:portfolio_type]
    assets = session[:selected_assets].keys.join('_')
    start_date = session[:backtest_start_date]
    cache_key = "smart_allocations_#{portfolio_type}_#{assets}_#{start_date}"
    expires_in = Utilities::Time.seconds_to_midnight_utc.seconds
    allocations = Rails.cache.fetch(cache_key, expires_in: expires_in) do
      client = FinancialDataApiClient.new
      symbols = session[:selected_assets].keys.map { |s| "#{s}/USDT" }.join(',')
      allocations_result = client.smart_allocations(symbols, start_date, portfolio_type)
      return if allocations_result.failure?

      allocations_result.data
    end

    session[:selected_assets].transform_values!.with_index { |_, i| allocations[session[:smart_allocation_selected]][i] }
  end

  def simulate_current_session
    # move to service
    return if session[:selected_assets].empty?

    portfolio_type = session[:portfolio_type]
    assets = session[:selected_assets].keys.join('_')
    allocations = session[:selected_assets].values.join('_')
    benchmark = session[:benchmark]
    start_date = session[:backtest_start_date]
    cache_key = "simulate_#{portfolio_type}_#{assets}_#{allocations}_#{benchmark}_#{start_date}"
    expires_in = Utilities::Time.seconds_to_midnight_utc.seconds
    metrics = Rails.cache.fetch(cache_key, expires_in: expires_in) do
      client = FinancialDataApiClient.new
      symbols = session[:selected_assets].keys.map { |s| "#{s}/USDT" }.join(',')
      metrics_result = client.metrics(symbols, allocations.gsub('_', ','), benchmark, start_date, portfolio_type)
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
