class PortfoliosController < ApplicationController
  layout 'analyzer'
  # before_action :initialize_session, only: [:show]
  before_action :set_portfolio

  def show
    @smart_allocations = @portfolio.get_smart_allocations if @portfolio.smart_allocation_on?
    @backtest_max_date = Time.now.utc - 2.days
    @benchmark_options = Portfolio.benchmarks.map { |k, _| [Portfolio::BENCHMARK_NAMES[k.to_sym], k] }
    @backtest = @portfolio.backtest
  end

  def update_benchmark
    new_benchmark = portfolio_params[:benchmark]
    return if @portfolio.benchmark == new_benchmark

    if Portfolio.benchmarks.include?(new_benchmark) && @portfolio.update(benchmark: new_benchmark)
      @backtest = @portfolio.backtest
      respond_to do |format|
        format.html { redirect_to portfolio_analyzer_path }
        format.turbo_stream { render 'refresh_backtest_results' }
      end
    else
      redirect_to portfolio_analyzer_path, alert: 'Invalid benchmark value.'
    end
  end

  def update_strategy
    new_strategy = portfolio_params[:strategy]
    return if @portfolio.strategy == new_strategy

    if Portfolio.strategies.include?(new_strategy) && @portfolio.update(strategy: new_strategy, smart_allocation_on: false)
      respond_to do |format|
        format.html { redirect_to portfolio_analyzer_path }
        format.turbo_stream { render 'refresh_allocations' }
      end
    else
      redirect_to portfolio_analyzer_path, alert: 'Invalid strategy value.'
    end
  end

  def update_backtest_start_date
    new_backtest_start_date = portfolio_params[:backtest_start_date]
    return if @portfolio.backtest_start_date == new_backtest_start_date

    if @portfolio.update(backtest_start_date: new_backtest_start_date, smart_allocation_on: false)
      respond_to do |format|
        format.html { redirect_to portfolio_analyzer_path }
        format.turbo_stream { render 'refresh_allocations' }
      end
    else
      redirect_to portfolio_analyzer_path, alert: 'Invalid date value.'
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
    @backtest = @portfolio.backtest
    render partial: 'backtest_results', locals: { backtest: @backtest }
  end

  private

  def portfolio_params
    params.require(:portfolio).permit(:benchmark, :strategy, :backtest_start_date, :risk_level, :smart_allocation_on)
  end

  def set_portfolio
    @portfolio = current_user.portfolios.first
    return if @portfolio.present?

    @portfolio = Portfolio.new(user: current_user, backtest_start_date: 1.year.ago.to_date.to_s)
    @portfolio.save
  end
end
