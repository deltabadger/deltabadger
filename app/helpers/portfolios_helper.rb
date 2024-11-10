module PortfoliosHelper
  def fill_default_portfolio_label(label)
    label || t('analyzer.default_portfolio')
  end

  def fill_default_portfolio_color(index)
    colors = ['#fd7f6f', '#7eb0d5', '#b2e061', '#bd7ebe', '#ffb55a', '#ffee65', '#beb9db', '#fdcce5', '#8bd3c7']
    colors[index % colors.size]
  end

  def backtest_metric_value(metric, percentage: false)
    return '-' if metric.nil?

    if percentage
      format('%.2f%%', (metric * 100))
    else
      format('%.2f', metric)
    end
  end

  def render_turbo_stream_backtest_results(backtest, portfolio)
    turbo_stream.replace 'backtest-results', partial: 'portfolios/backtest_results', locals: {
      backtest: backtest,
      portfolio: portfolio
    }
  end

  def render_turbo_stream_portfolio_assets(portfolio, last_active_assets_ids, last_idle_assets_ids)
    active_assets = portfolio.active_assets
    idle_assets = portfolio.idle_assets
    active_assets_ids = active_assets.map(&:id)
    idle_assets_ids = idle_assets.map(&:id)

    streams = []
    streams.concat generate_remove_streams(active_assets_ids, last_active_assets_ids, 'active')
    streams.concat generate_add_or_replace_streams(active_assets, active_assets_ids, last_active_assets_ids, 'active')
    streams.concat generate_remove_streams(idle_assets_ids, last_idle_assets_ids, 'idle')
    streams.concat generate_add_or_replace_streams(idle_assets, idle_assets_ids, last_idle_assets_ids, 'idle')

    streams << handle_idle_assets_label(idle_assets_ids, last_idle_assets_ids, portfolio)
    streams.compact.join.html_safe
  end

  private

  def generate_remove_streams(current_ids, last_ids, prefix)
    (last_ids - current_ids).map do |id|
      turbo_stream.remove("#{prefix}_asset_#{id}")
    end
  end

  def generate_add_or_replace_streams(assets, current_ids, last_ids, prefix)
    assets.map.with_index do |asset, index|
      if (current_ids - last_ids).include?(asset.id)
        if index.zero?
          turbo_stream.prepend("portfolio-#{prefix}-assets", partial: 'assets/asset', locals: { asset: asset })
        else
          after_asset = assets[index - 1]
          turbo_stream.after("#{prefix}_asset_#{after_asset.id}", partial: 'assets/asset', locals: { asset: asset })
        end
      else
        turbo_stream.replace("#{prefix}_asset_#{asset.id}", partial: 'assets/asset', locals: { asset: asset })
      end
    end
  end

  def handle_idle_assets_label(idle_assets_ids, last_idle_assets_ids, portfolio)
    if idle_assets_ids.any? && last_idle_assets_ids.none?
      turbo_stream.before('portfolio-idle-assets', partial: 'portfolios/idle_assets_label', locals: { portfolio: portfolio })
    elsif idle_assets_ids.none? && last_idle_assets_ids.any?
      turbo_stream.remove('portfolio-idle-assets-label')
    end
  end
end
