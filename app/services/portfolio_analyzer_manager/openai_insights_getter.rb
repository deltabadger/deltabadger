require 'utilities/time'

module PortfolioAnalyzerManager
  class OpenaiInsightsGetter < BaseService
    def call(portfolio)
      expires_in = Utilities::Time.seconds_to_midnight_utc.seconds + 5.minutes
      cache_key = 'openai_insights_' + portfolio.backtest_cache_key
      Rails.cache.fetch(cache_key, expires_in: expires_in) do
        messages = training_prompts.map { |prompt| { role: 'system', content: prompt } }
        messages << { role: 'user', content: prompt(portfolio) }
        response = client.chat(
          parameters: {
            model: 'gpt-4o-mini',
            messages: messages,
            temperature: 0.7
          }
        )
        answer = response.dig('choices', 0, 'message', 'content')
        return Result::Failure.new(response) if answer.blank?

        Result::Success.new(answer)
      end
    end

    private

    def client
      @_client ||= OpenAI::Client.new
    end

    def training_prompts
      [
        ''
      ]
    end

    def prompt(portfolio)
      text = 'Evaluate my portfolio like Naval. Show up to 5 pros and/or cons. Organize them: pros first. Put more attention to metrics like volatility and beta, then those mostly reflecting past like Sharpe. Find geopolitical or technical thesis behind. Notice relevant macro events in the recent years. Point out overlapping assets like ETFs with the same stocks. Use Poor/Neutral/Good/Exeptional scale. Reply in the following HTML format (all texts are examples). I inject your reply directly into my HTML template so do NOT return it as a code snippet: <div class="insight insight--summary"><span class="insight__name">Edgy title</span>. Short narrative about the portfolio in 5 sentences.</div><div class="insight insight--pro"><span class="insight__name">Sharpe Ratio <b>2.22</b> &middot; Exceptional</span>. Excellent risk-adjusted returns.</div><div class="insight insight--con"><span class="insight__name">CVaR <b>-1.36%</b> &middot; Poor</span>. The conditional value at risk is… not small.</div>'
      text += ' '
      text += 'Assets:'
      portfolio.assets.each do |asset|
        text += " #{asset.ticker} #{(asset.effective_allocation * 100).round(1)}%"
      end
      text += '. '
      text += "Benchmark: #{portfolio.benchmark_name}. "
      text += "Risk-free rate: #{(portfolio.risk_free_rate * 100).round(1)}%. "
      text += "Metrics for time since #{portfolio.backtest_start_date} to #{1.day.ago.to_date}: "
      text += "Past performance +#{(portfolio.backtest['metrics']['totalReturn'] * 100).round(0)}%, "
      text += "Benchmark past performance +#{(portfolio.backtest['metrics']['benchmarkTotalReturn'] * 100).round(0)}%, "
      text += "Expected Return #{(portfolio.backtest['metrics']['expectedReturn'] * 100).round(1)}%, "
      text += "CAGR #{(portfolio.backtest['metrics']['cagr'] * 100).round(1)}%, "
      text += "Volatility #{(portfolio.backtest['metrics']['volatility'] * 100).round(1)}%, "
      text += "Max. Drawdown #{(portfolio.backtest['metrics']['maxDrawdown'] * 100).round(1)}%, "
      text += "Calmar Ratio #{portfolio.backtest['metrics']['calmarRatio'].round(2)}, "
      text += "VaR #{(portfolio.backtest['metrics']['valueAtRisk'] * 100).round(1)}%, "
      text += "CVaR #{(portfolio.backtest['metrics']['conditionalValueAtRisk'] * 100).round(1)}%, "
      text += "Sharpe Ratio #{portfolio.backtest['metrics']['sharpeRatio'].round(2)}, "
      text += "Sortino Ratio #{portfolio.backtest['metrics']['sortinoRatio'].round(2)}, "
      text += "Treynor Ratio #{portfolio.backtest['metrics']['treynorRatio'].round(2)}, "
      text += "Omega Ratio #{portfolio.backtest['metrics']['omegaRatio'].round(2)}, "
      text += "Alpha #{portfolio.backtest['metrics']['alpha'].round(2)}, "
      text += "Beta #{portfolio.backtest['metrics']['beta'].round(2)}, "
      text += "R-squared #{portfolio.backtest['metrics']['rSquared'].round(2)}, "
      text += "Information Ratio #{portfolio.backtest['metrics']['informationRatio'].round(2)}"
      text
    end
  end
end
