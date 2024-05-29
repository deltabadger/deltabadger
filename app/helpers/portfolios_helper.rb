module PortfoliosHelper
  def render_turbo_stream_backtest_results(backtest, portfolio)
    turbo_stream.replace 'backtest-results', partial: 'portfolios/backtest_results', locals: {
      backtest: backtest,
      portfolio: portfolio
    }
  end

  def render_turbo_stream_portfolio_assets(portfolio)
    portfolio.assets.map do |asset|
      turbo_stream.replace asset, partial: 'assets/asset', locals: { asset: asset }
    end.join.html_safe
  end
end
