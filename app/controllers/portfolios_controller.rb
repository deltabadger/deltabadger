class PortfoliosController < ApplicationController
  layout 'analyzer'
  before_action :authenticate_user!
  before_action :set_portfolio
  before_action :set_last_assets, except: %i[update_risk_level]
  after_action :save_last_assets, except: %i[update_risk_level normalize_allocations]

  def show
    set_backtest_data
  end

  def update_benchmark
    new_benchmark = portfolio_params[:benchmark]
    return head :ok if @portfolio.benchmark == new_benchmark

    if Portfolio.benchmarks.include?(new_benchmark) && @portfolio.update(benchmark: new_benchmark)
      set_backtest_data
      respond_to do |format|
        format.html { redirect_to portfolio_analyzer_path }
        format.turbo_stream
      end
    else
      flash.now[:alert] = 'Invalid benchmark value.'
      respond_to do |format|
        format.turbo_stream { render turbo_stream: render_turbo_stream_flash_messages, status: :unprocessable_entity }
        format.html { redirect_to portfolio_analyzer_path, alert: 'Invalid benchmark value.' }
      end
    end
  end

  def update_strategy
    new_strategy = portfolio_params[:strategy]
    return head :ok if @portfolio.strategy == new_strategy

    if Portfolio.strategies.include?(new_strategy) && @portfolio.update(strategy: new_strategy)
      set_backtest_data
      respond_to do |format|
        format.html { redirect_to portfolio_analyzer_path }
        format.turbo_stream { render 'update_benchmark' }
      end
    else
      flash.now[:alert] = 'Invalid strategy value.'
      respond_to do |format|
        format.turbo_stream { render turbo_stream: render_turbo_stream_flash_messages, status: :unprocessable_entity }
        format.html { redirect_to portfolio_analyzer_path, alert: 'Invalid strategy value.' }
      end
    end
  end

  def update_backtest_start_date
    new_backtest_start_date = portfolio_params[:backtest_start_date]
    return head :ok if @portfolio.backtest_start_date == new_backtest_start_date

    if @portfolio.update(backtest_start_date: new_backtest_start_date)
      set_backtest_data
      respond_to do |format|
        format.html { redirect_to portfolio_analyzer_path }
        format.turbo_stream
      end
    else
      flash.now[:alert] = 'Invalid start date value.'
      respond_to do |format|
        format.turbo_stream { render turbo_stream: render_turbo_stream_flash_messages, status: :unprocessable_entity }
        format.html { redirect_to portfolio_analyzer_path, alert: 'Invalid start date value.' }
      end
    end
  end

  def update_risk_free_rate
    new_risk_free_rate = if params[:risk_free_rate_shortcut].present?
                           @portfolio.get_risk_free_rate(params[:risk_free_rate_shortcut])
                         else
                           portfolio_params[:risk_free_rate].to_f / 100
                         end
    return head :ok if @portfolio.risk_free_rate == new_risk_free_rate

    if @portfolio.update(risk_free_rate: new_risk_free_rate)
      set_backtest_data
      respond_to do |format|
        format.html { redirect_to portfolio_analyzer_path }
        format.turbo_stream
      end
    else
      flash.now[:alert] = 'Invalid risk free rate value.'
      respond_to do |format|
        format.turbo_stream { render turbo_stream: render_turbo_stream_flash_messages, status: :unprocessable_entity }
        format.html { redirect_to portfolio_analyzer_path, alert: 'Invalid risk free rate value.' }
      end
    end
  end

  def toggle_smart_allocation
    if @portfolio.update(smart_allocation_on: portfolio_params[:smart_allocation_on])
      set_backtest_data
      respond_to do |format|
        format.turbo_stream
        format.html { redirect_to portfolio_analyzer_path, notice: 'Smart allocations have been updated.' }
      end
    else
      flash.now[:alert] = 'Invalid smart allocation value.'
      respond_to do |format|
        format.turbo_stream { render turbo_stream: render_turbo_stream_flash_messages, status: :unprocessable_entity }
        format.html { redirect_to portfolio_analyzer_path, alert: 'Invalid smart allocation value.' }
      end
    end
  end

  def update_risk_level
    new_risk_level = Portfolio.risk_levels.key(portfolio_params[:risk_level].to_i)
    return head :ok if @portfolio.risk_level == new_risk_level

    if Portfolio.risk_levels.include?(new_risk_level) && @portfolio.update(risk_level: new_risk_level)
      set_backtest_data
      respond_to do |format|
        format.turbo_stream
        format.html { redirect_to portfolio_analyzer_path, notice: 'Risk level has been updated.' }
      end
    else
      flash.now[:alert] = 'Invalid risk level value.'
      respond_to do |format|
        format.turbo_stream { render turbo_stream: render_turbo_stream_flash_messages, status: :unprocessable_entity }
        format.html { redirect_to portfolio_analyzer_path, alert: 'Invalid risk level value.' }
      end
    end
  end

  def normalize_allocations
    @portfolio.normalize_allocations!
    @backtest = @portfolio.backtest
    respond_to do |format|
      format.turbo_stream
      format.html { redirect_to portfolio_analyzer_path, notice: 'Portfolio allocations have been normalized.' }
    end
  end

  private

  def portfolio_params
    params.require(:portfolio).permit(
      :benchmark,
      :strategy,
      :backtest_start_date,
      :risk_level,
      :smart_allocation_on,
      :risk_free_rate
    )
  end

  def set_portfolio
    @portfolio = current_user.portfolios.first
    return if @portfolio.present?

    @portfolio = Portfolio.new(
      user: current_user,
      backtest_start_date: 1.year.ago.to_date.to_s,
      risk_free_rate: 0.0435
    )
    @portfolio.save
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
