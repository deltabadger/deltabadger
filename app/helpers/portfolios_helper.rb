module PortfoliosHelper
  def render_turbo_stream_backtest_results(backtest, portfolio)
    turbo_stream.replace 'backtest-results', partial: 'portfolios/backtest_results', locals: {
      backtest: backtest,
      portfolio: portfolio
    }
  end

  def render_turbo_stream_portfolio_assets(portfolio, ignore_asset = nil)
    streams = portfolio.assets.map do |asset|
      next if asset == ignore_asset

      turbo_stream.replace asset, partial: 'assets/asset', locals: { asset: asset }
    end
    streams << turbo_stream.replace('portfolio-idle-assets-label', partial: 'portfolios/idle_assets_label', locals: {
                                      portfolio: portfolio
                                    })
    streams.compact.join.html_safe
  end
end
