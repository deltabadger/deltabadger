class PortfoliosController < ApplicationController
  include ApplicationHelper

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
      flash.now[:alert] = t('alert.portfolio.invalid_benchmark')
      respond_to do |format|
        format.turbo_stream { render turbo_stream: render_turbo_stream_flash_messages, status: :unprocessable_entity }
        format.html { redirect_to portfolio_analyzer_path, alert: t('alert.portfolio.invalid_benchmark') }
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
      flash.now[:alert] = t('alert.portfolio.invalid_strategy')
      respond_to do |format|
        format.turbo_stream { render turbo_stream: render_turbo_stream_flash_messages, status: :unprocessable_entity }
        format.html { redirect_to portfolio_analyzer_path, alert: t('alert.portfolio.invalid_strategy') }
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
      flash.now[:alert] = t('alert.portfolio.invalid_start_date')
      respond_to do |format|
        format.turbo_stream { render turbo_stream: render_turbo_stream_flash_messages, status: :unprocessable_entity }
        format.html { redirect_to portfolio_analyzer_path, alert: t('alert.portfolio.invalid_start_date') }
      end
    end
  end

  def update_risk_free_rate
    if params[:risk_free_rate_shortcut].present?
      risk_free_rate_result = @portfolio.get_risk_free_rate(params[:risk_free_rate_shortcut])
      if risk_free_rate_result.failure?
        flash.now[:alert] = risk_free_rate_result.errors.first
        respond_to do |format|
          format.turbo_stream { render turbo_stream: render_turbo_stream_flash_messages, status: :unprocessable_entity }
          format.html { redirect_to portfolio_analyzer_path, alert: risk_free_rate_result.errors.first }
        end
        return
      else
        new_risk_free_rate = risk_free_rate_result.data
      end
    else
      new_risk_free_rate = portfolio_params[:risk_free_rate].to_f / 100
    end
    return head :ok if @portfolio.risk_free_rate == new_risk_free_rate

    if @portfolio.update(risk_free_rate: new_risk_free_rate)
      set_backtest_data
      respond_to do |format|
        format.html { redirect_to portfolio_analyzer_path }
        format.turbo_stream
      end
    else
      flash.now[:alert] = t('alert.portfolio.invalid_risk_free_rate')
      respond_to do |format|
        format.turbo_stream { render turbo_stream: render_turbo_stream_flash_messages, status: :unprocessable_entity }
        format.html { redirect_to portfolio_analyzer_path, alert: t('alert.portfolio.invalid_risk_free_rate') }
      end
    end
  end

  def toggle_smart_allocation
    if @portfolio.update(smart_allocation_on: portfolio_params[:smart_allocation_on])
      puts "smart allocation on: #{portfolio_params[:smart_allocation_on]}, @portfolio.smart_allocation_on: #{@portfolio.smart_allocation_on}"
      set_backtest_data
      respond_to do |format|
        format.turbo_stream
        format.html { redirect_to portfolio_analyzer_path, notice: t('alert.portfolio.smart_allocation_updated') }
      end
    else
      flash.now[:alert] = t('alert.portfolio.invalid_smart_allocation')
      respond_to do |format|
        format.turbo_stream { render turbo_stream: render_turbo_stream_flash_messages, status: :unprocessable_entity }
        format.html { redirect_to portfolio_analyzer_path, alert: t('alert.portfolio.invalid_smart_allocation') }
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
        format.html { redirect_to portfolio_analyzer_path, notice: t('alert.portfolio.risk_level_updated') }
      end
    else
      flash.now[:alert] = t('alert.portfolio.invalid_risk_level')
      respond_to do |format|
        format.turbo_stream { render turbo_stream: render_turbo_stream_flash_messages, status: :unprocessable_entity }
        format.html { redirect_to portfolio_analyzer_path, alert: t('alert.portfolio.invalid_risk_level') }
      end
    end
  end

  def normalize_allocations
    @portfolio.normalize_allocations!
    @backtest = @portfolio.backtest
    respond_to do |format|
      format.turbo_stream
      format.html { redirect_to portfolio_analyzer_path, notice: t('alert.portfolio.portfolio_normalized') }
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
    @backtest = @portfolio.backtest if @portfolio.allocations_are_normalized?
    # return unless @portfolio.smart_allocation_on? && !@portfolio.allocations_are_smart?

    # show flash message if the data API server is unreachable.
    # flash.now[:alert] = t('alert.portfolio.unable_to_calculate')
  end

  def set_last_assets
    @last_active_assets_ids = session[:last_active_assets_ids] || []
    @last_idle_assets_ids = session[:last_idle_assets_ids] || []
  end

  def save_last_assets
    session[:last_active_assets_ids] = @portfolio.active_assets.map(&:id)
    session[:last_idle_assets_ids] = @portfolio.idle_assets.map(&:id)
  end

  def parse_risk_free_rate_params
    if params[:risk_free_rate_shortcut].present?
      risk_free_rate_result = @portfolio.get_risk_free_rate(params[:risk_free_rate_shortcut])
      if risk_free_rate_result.errors.first != 'Invalid Risk Free Rate Key.'
        risk_free_rate_name = Portfolio::RISK_FREE_RATES[params[:risk_free_rate_shortcut].to_sym][:name]
        flash.now[:alert] = "#{t('alert.portfolio.unable_to_set_risk_rate', risk_free_rate_name: risk_free_rate_name)} #{t('alert.portfolio.api_unreachable')}"
        nil
      else
        risk_free_rate_result.data
      end
    elsif portfolio_params[:risk_free_rate].present?
      portfolio_params[:risk_free_rate].to_f / 100
    end
  end
end