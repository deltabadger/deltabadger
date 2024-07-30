require 'utilities/time'

module PortfolioAnalyzerManager
  class OpenaiInsightsGetter < BaseService
    def call(portfolio)
      expires_in = Utilities::Time.seconds_to_midnight_utc.seconds + 5.minutes
      cache_key = 'openai_insights_' + portfolio.backtest_cache_key
      Rails.cache.fetch(cache_key, expires_in: expires_in) do
        messages = training_prompts.map { |prompt| { role: "system", content: prompt } }
        messages << { role: "user", content: prompt(portfolio) }
        response = client.chat(
          parameters: {
            model: "gpt-4o",
            messages: messages,
            temperature: 0.7,
          }
        )
        answer = response.dig("choices", 0, "message", "content")
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
        "You are a financial advisor explaining things to a client who is new to investing",
      ]
    end

    def prompt(portfolio)
      text = ''
      text += 'This is my portfolio: '
      text += 'Assets:'
      portfolio.assets.each do |asset|
        text += " #{asset.ticker} #{(asset.effective_allocation * 100).round(2)}%"
      end
      text += '. '
      text += "Benchmark: #{portfolio.benchmark_name}. "
      text += "Risk-free rate: #{(portfolio.risk_free_rate * 100).round(2)}%. "
      text += "Metrics for time since #{portfolio.backtest_start_date} to #{1.day.ago.to_date}: "
      text += "Portfolio performance +#{portfolio.backtest['metrics']['totalReturn'].round(2)}%, "
      text += "Benchmark performance +#{portfolio.backtest['metrics']['benchmarkTotalReturn'].round(2)}%, "
      text += "Expected Return #{portfolio.backtest['metrics']['expectedReturn'].round(2)}%, "
      text += "CAGR #{portfolio.backtest['metrics']['cagr'].round(2)}%, "
      text += "Volatility #{portfolio.backtest['metrics']['volatility'].round(2)}%, "
      text += "Max. Drawdown #{portfolio.backtest['metrics']['maxDrawdown'].round(2)}%, "
      text += "Calmar Ratio #{portfolio.backtest['metrics']['calmarRatio'].round(2)}, "
      text += "VaR #{portfolio.backtest['metrics']['valueAtRisk'].round(2)}%, "
      text += "CVaR #{portfolio.backtest['metrics']['conditionalValueAtRisk'].round(2)}%, "
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
