module PortfoliosHelper
  def render_turbo_stream_backtest_results(backtest, portfolio)
    turbo_stream.replace 'backtest-results', partial: 'portfolios/backtest_results', locals: {
      backtest: backtest,
      portfolio: portfolio
    }
  end

  # rubocop:disable Metrics/CyclomaticComplexity,Metrics/PerceivedComplexity,Metrics/MethodLength
  def render_turbo_stream_portfolio_assets(portfolio, last_active_assets_ids, last_idle_assets_ids)
    active_assets = portfolio.active_assets
    idle_assets = portfolio.idle_assets
    active_assets_ids = active_assets.map(&:id)
    idle_assets_ids = idle_assets.map(&:id)

    active_assets_ids_to_remove = last_active_assets_ids - active_assets_ids
    active_assets_ids_to_add = active_assets_ids - last_active_assets_ids

    idle_assets_ids_to_remove = last_idle_assets_ids - idle_assets_ids
    idle_assets_ids_to_add = idle_assets_ids - last_idle_assets_ids

    streams = []

    active_assets_ids_to_remove.each do |asset_id|
      streams << turbo_stream.remove("active_asset_#{asset_id}")
    end
    idle_assets_ids_to_remove.each do |asset_id|
      streams << turbo_stream.remove("idle_asset_#{asset_id}")
    end

    active_assets.each_with_index do |asset, index|
      if active_assets_ids_to_add.include?(asset.id)
        if index.zero?
          streams << turbo_stream.append('portfolio-active-assets', partial: 'assets/asset', locals: { asset: asset })
        else
          after_asset = active_assets[index - 1]
          streams << turbo_stream.after("active_asset_#{after_asset.id}", partial: 'assets/asset', locals: { asset: asset })
        end
      else
        streams << turbo_stream.replace("active_asset_#{asset.id}", partial: 'assets/asset', locals: { asset: asset })
      end
    end

    idle_assets.each_with_index do |asset, index|
      if idle_assets_ids_to_add.include?(asset.id)
        if index.zero?
          streams << turbo_stream.append('portfolio-idle-assets', partial: 'assets/asset', locals: { asset: asset })
        else
          after_asset = idle_assets[index - 1]
          streams << turbo_stream.after("idle_asset_#{after_asset.id}", partial: 'assets/asset', locals: { asset: asset })
        end
      end
    end

    if idle_assets_ids.any? && last_idle_assets_ids.none?
      streams << turbo_stream.before('portfolio-idle-assets', partial: 'portfolios/idle_assets_label', locals: { portfolio: portfolio })
    elsif idle_assets_ids.none? && last_idle_assets_ids.any?
      streams << turbo_stream.remove('portfolio-idle-assets-label')
    end
    streams.join.html_safe
  end
  # rubocop:enable Metrics/CyclomaticComplexity,Metrics/PerceivedComplexity,Metrics/MethodLength
end
