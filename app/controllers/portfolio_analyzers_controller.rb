class PortfolioAnalyzersController < ApplicationController
  before_action :set_assets, only: %i[show add_asset remove_asset]

  def show
    simulate_current_session
    session[:query] = params[:query]
    @query_assets = get_query_assets(session[:query])
    @smart_allocations_enabled = session[:smart_allocations_enabled] || false

    if turbo_frame_request?
      render partial: 'asset_selector', locals: { query_assets: @query_assets }
    else
      render :show
    end

    # respond_to do |format|
    #   format.turbo_frame { render partial: 'asset_selector', locals: { query_assets: @query_assets } }
    #   format.html { render :show }
    # end
  end

  def add_asset
    @asset = add_remove_params[:asset]
    @allocation = 0 # Default allocation to 0
    if !session[:selected_assets].key?(@asset) && @all_assets.include?(@asset)
      session[:selected_assets][@asset] = @allocation
      @available_assets -= [@asset]
      @query_assets = get_query_assets(session[:query])
      @smart_allocations_enabled = false
    else
      flash[:alert] = 'Invalid asset'
      return redirect_to portfolio_analyzer_path
    end

    respond_to do |format|
      format.turbo_stream
      format.html { redirect_to portfolio_analyzer_path }
    end
  end

  def remove_asset
    @asset = add_remove_params[:asset]
    if session[:selected_assets].key?(@asset)
      session[:selected_assets].delete(@asset)
      @available_assets += [@asset]
      @query_assets = get_query_assets(session[:query])
      @smart_allocations_enabled = false
    else
      flash[:alert] = 'Invalid asset'
      return redirect_to portfolio_analyzer_path
    end

    respond_to do |format|
      format.turbo_stream
      format.html { redirect_to portfolio_analyzer_path }
    end
  end

  def update_allocation
    @asset = update_allocation_params[:asset]
    @allocation = params[:allocation].to_f
    if session[:selected_assets].key?(@asset)
      session[:selected_assets][@asset] = @allocation
    else
      flash[:alert] = 'Invalid asset'
      redirect_to portfolio_analyzer_path
    end
    @selected_assets = session[:selected_assets]

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
    @selected_assets = session[:selected_assets]

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
    get_smart_allocations
    @selected_assets = session[:selected_assets]
    @smart_allocations_enabled = session[:smart_allocations_enabled]

    respond_to do |format|
      format.turbo_stream { render :normalize_allocations }
      format.html { redirect_to portfolio_analyzer_path }
    end
  end

  private

  def set_defaults; end

  def set_assets
    session[:selected_assets] ||= {}
    @selected_assets = session[:selected_assets]
    @all_assets = all_symbols.map { |s| s[0...-4] }.sort!
    @available_assets = @all_assets - @selected_assets.keys
  end

  def add_remove_params
    params.permit(:asset)
  end

  def update_allocation_params
    params.permit(:asset, :allocation)
  end

  def seconds_to_midnight_utc
    now = Time.now.utc
    midnight = now.end_of_day + 1.second
    midnight - now
  end

  def all_symbols
    source = 'binance'
    timeframe = '1d'
    Rails.cache.fetch("symbols_#{source}_#{timeframe}", expires_in: seconds_to_midnight_utc.seconds) do
      client = FinancialDataApiClient.new
      puts "Fetching symbols from #{source} for #{timeframe}"
      symbols_result = client.symbols('binance', '1d')
      return {} if symbols_result.failure?

      symbols_result.data
    end
  end

  def get_query_assets(query)
    set_assets
    return [] if query.blank?

    @available_assets.filter { |a| a.include?(query.upcase) }
  end

  def get_smart_allocations
    assets = session[:selected_assets].keys.join('_')
    start_date = '2021-01-01'
    portfolio_type = 'fixed'
    cache_key = "smart_allocations_#{portfolio_type}_#{assets}_#{start_date}"
    allocations = Rails.cache.fetch(cache_key, expires_in: seconds_to_midnight_utc.seconds) do
      client = FinancialDataApiClient.new
      puts 'Fetching smart allocations'
      symbols = session[:selected_assets].keys.map { |s| "#{s}/USDT" }.join(',')
      allocations_result = client.smart_allocations(symbols, start_date, portfolio_type)
      puts 'wtf0'
      return if allocations_result.failure?

      puts 'wtf1'
      allocations_result.data
    end

    session[:selected_assets].transform_values!.with_index { |_, i| allocations[2][i] }
  end

  def simulate_current_session
    @metrics = {}
    @data_labels = []
    @data_series = []
    return if session[:selected_assets].empty?

    assets = session[:selected_assets].keys.join('_')
    allocations = session[:selected_assets].values.join('_')
    start_date = '2021-01-01'
    portfolio_type = 'fixed'
    benchmark = '^GSPC'
    cache_key = "simulate_#{portfolio_type}_#{assets}_#{allocations}_#{benchmark}_#{start_date}"
    metrics = Rails.cache.fetch(cache_key, expires_in: seconds_to_midnight_utc.seconds) do
      client = FinancialDataApiClient.new
      puts 'Fetching simulation'
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
