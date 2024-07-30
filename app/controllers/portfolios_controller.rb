class PortfoliosController < ApplicationController
  include ApplicationHelper

  layout 'analyzer'
  before_action :authenticate_user!
  before_action :set_portfolio, except: %i[new create]
  before_action :set_last_assets, except: %i[update_risk_level]
  after_action :save_last_assets, except: %i[update_risk_level normalize_allocations]

  def show
    @portfolios = current_user.portfolios.all
    set_backtest_data
  end

  def new
    @portfolio = Portfolio.new
  end

  def create
    @portfolio = Portfolio.new(default_portfolio_params.merge(portfolio_params))

    if @portfolio.save
      redirect_to portfolio_path(@portfolio), notice: t('alert.portfolio.portfolio_created')
    else
      render :new
    end
  end

  def edit; end

  def update
    if @portfolio.update(portfolio_params)
      redirect_to portfolio_path(@portfolio), notice: t('alert.portfolio.portfolio_updated')
    else
      render :edit
    end
  end

  def destroy
    @portfolio.destroy
    session[:portfolio_id] = nil
    redirect_to portfolios_path, notice: t('alert.portfolio.portfolio_destroyed')
  end

  def duplicate
    ActiveRecord::Base.transaction do
      new_portfolio = @portfolio.dup
      new_portfolio.label = "#{new_portfolio.label} copy"
      raise ActiveRecord::Rollback, 'Portfolio duplication failed' unless new_portfolio.save

      @portfolio.assets.each do |asset|
        new_asset = asset.dup
        new_asset.portfolio = new_portfolio
        raise ActiveRecord::Rollback, 'Asset duplication failed' unless new_asset.save
      end
      @portfolio = new_portfolio
    end

    if @portfolio.persisted?
      render :edit, notice: t('alert.portfolio.portfolio_duplicated')
    else
      redirect_to portfolio_analyzer_path, alert: t('alert.portfolio.unable_to_duplicate_portfolio')
    end
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

  def openai_insights
    insights_result = PortfolioAnalyzerManager::OpenaiInsightsGetter.call(@portfolio)
    if insights_result.success?
      @insights = insights_result.data
      respond_to do |format|
        format.turbo_stream
        format.html { redirect_to portfolio_analyzer_path } # TODO: redirect to the correct place
      end
    else
      flash.now[:alert] = "Unable to get insights."
      respond_to do |format|
        format.turbo_stream { render turbo_stream: render_turbo_stream_flash_messages, status: :unprocessable_entity }
        format.html { redirect_to portfolio_analyzer_path, alert: "Unable to get insights." }
      end
    end
  end

  private

  def portfolio_params
    params.require(:portfolio).permit(
      :label,
      :benchmark,
      :strategy,
      :backtest_start_date,
      :risk_level,
      :smart_allocation_on,
      :risk_free_rate
    )
  end

  def set_portfolio
    @portfolio = if params.present? && params[:id].present?
                   current_user.portfolios.find(params[:id])
                 elsif params.present? && params[:portfolio_id].present?
                   current_user.portfolios.find(params[:portfolio_id])
                 elsif session[:portfolio_id].present?
                   current_user.portfolios.find(session[:portfolio_id])
                 else
                   current_user.portfolios.first
                 end
    unless @portfolio.present?
      @portfolio = Portfolio.new(default_portfolio_params)
      @portfolio.save
    end
    session[:portfolio_id] = @portfolio.id
  end

  def default_portfolio_params
    {
      user: current_user,
      backtest_start_date: 1.year.ago.to_date.to_s,
      risk_free_rate: 0.0435
    }
  end

  def set_backtest_data
    @backtest = @portfolio.backtest if @portfolio.allocations_are_normalized?
    return unless @portfolio.smart_allocation_on? && @portfolio.assets.present? && @portfolio.smart_allocations[0].empty?

    # show flash message if the data API server is unreachable.
    flash.now[:alert] = t('alert.portfolio.unable_to_calculate')
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
        flash.now[:alert] =
          "#{t('alert.portfolio.unable_to_set_risk_rate',
               risk_free_rate_name: risk_free_rate_name)} #{t('alert.portfolio.api_unreachable')}"
        nil
      else
        risk_free_rate_result.data
      end
    elsif portfolio_params[:risk_free_rate].present?
      portfolio_params[:risk_free_rate].to_f / 100
    end
  end
end
