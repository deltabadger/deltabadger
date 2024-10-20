require 'utilities/time'

module PortfolioAnalyzerManager
  class OpenaiInsightsGetter < BaseService
    def call(portfolio)
      expires_in = Utilities::Time.seconds_to_midnight_utc.seconds + 5.minutes
      cache_key = 'openai_insights_' + portfolio.backtest_cache_key
      Rails.cache.fetch(cache_key, expires_in: expires_in) do
        messages = training_prompts.map { |prompt| { role: 'system', content: prompt } }
        final_prompt = prompt(portfolio)
        messages << { role: 'user', content: prompt(portfolio) }

        # Log the final prompt to Rails log
        Rails.logger.info "Final OpenAI prompt: #{final_prompt}"

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
      text = I18n.t('ai.insights.intro') + ' '
      text += I18n.t('ai.insights.response_format') + ' '
      text += ' '
      text += I18n.t('utils.assets') + ':'
      portfolio.assets.each do |asset|
        text += " #{asset.ticker} #{(asset.effective_allocation * 100).round(1)}%"
      end
      text += '. '
      text += "#{I18n.t('analyzer.benchmark')} #{portfolio.benchmark_name}. "
      text += "#{I18n.t('analyzer.risk_free_rate')} #{(portfolio.risk_free_rate * 100).round(1)}%. "
      text += "#{I18n.t('ai.insights.metrics_since', start_date: portfolio.backtest_start_date,
                                                     end_date: 1.day.ago.to_date)} "
      text += "#{I18n.t('ai.insights.past_performance')} +#{(portfolio.backtest['metrics']['totalReturn'] * 100).round(0)}%, "
      text += "#{I18n.t('ai.insights.benchmark_performance')} +#{(portfolio.backtest['metrics']['benchmarkTotalReturn'] * 100).round(0)}%, "
      text += I18n.t('metrics.exp_return.short_name') + ": #{(portfolio.backtest['metrics']['expectedReturn'] * 100).round(1)}%, "
      text += I18n.t('metrics.cagr.short_name') + ": #{(portfolio.backtest['metrics']['cagr'] * 100).round(1)}%, "
      text += I18n.t('metrics.volatility.short_name') + ": #{(portfolio.backtest['metrics']['volatility'] * 100).round(1)}%, "
      text += I18n.t('metrics.max_drawdown.short_name') + ": #{(portfolio.backtest['metrics']['maxDrawdown'] * 100).round(1)}%, "
      text += I18n.t('metrics.calmar_ratio.short_name') + ": #{portfolio.backtest['metrics']['calmarRatio'].round(2)}, "
      text += I18n.t('metrics.var.short_name') + ": #{(portfolio.backtest['metrics']['valueAtRisk'] * 100).round(1)}%, "
      text += I18n.t('metrics.cvar.short_name') + ": #{(portfolio.backtest['metrics']['conditionalValueAtRisk'] * 100).round(1)}%, "
      text += I18n.t('metrics.sharpe_ratio.short_name') + ": #{portfolio.backtest['metrics']['sharpeRatio'].round(2)}, "
      text += I18n.t('metrics.sortino_ratio.short_name') + ": #{portfolio.backtest['metrics']['sortinoRatio'].round(2)}, "
      text += I18n.t('metrics.treynor_ratio.short_name') + ": #{portfolio.backtest['metrics']['treynorRatio'].round(2)}, "
      text += I18n.t('metrics.omega_ratio.short_name') + ": #{portfolio.backtest['metrics']['omegaRatio'].round(2)}, "
      text += I18n.t('metrics.alpha.short_name') + ": #{portfolio.backtest['metrics']['alpha'].round(2)}, "
      text += I18n.t('metrics.beta.short_name') + ": #{portfolio.backtest['metrics']['beta'].round(2)}, "
      text += I18n.t('metrics.r_squared.short_name') + ": #{portfolio.backtest['metrics']['rSquared'].round(2)}, "
      text += I18n.t('metrics.info_ratio.short_name') + ": #{portfolio.backtest['metrics']['informationRatio'].round(2)}"
      text
    end
  end
end
