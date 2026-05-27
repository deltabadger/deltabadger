# frozen_string_literal: true

module BotApi
  module Portfolio
    # Aggregates global P/L + per-bot metrics for the dashboard view.
    # `empty: true` is surfaced when the user has no bots so the MCP
    # presenter can still produce its existing "create a bot" copy.
    class Summary
      def self.call(user:)
        new(user: user).call
      end

      def initialize(user:)
        @user = user
      end

      def call
        bots = @user.bots.not_deleted
        return Result.success({ empty: true, bots: [], totals: nil, global_pnl: nil }) if bots.empty?

        Result.success({
                         empty: false,
                         totals: totals_for(bots),
                         global_pnl: global_pnl_for(@user),
                         bots: bots.map { |bot| per_bot(bot) }
                       })
      end

      private

      def totals_for(bots)
        {
          total: bots.size,
          working: bots.count(&:working?),
          stopped: bots.where(status: :stopped).count,
          created: bots.where(status: :created).count
        }
      end

      def global_pnl_for(user)
        pnl = user.global_pnl(use_cache: true)
        return nil unless pnl

        { percent: pnl[:percent], profit_usd: pnl[:profit_usd] }
      end

      def per_bot(bot)
        quote = bot.quote_asset&.symbol
        base = bot.respond_to?(:base_asset) ? bot.base_asset&.symbol : nil
        metrics, error = safe_metrics(bot)

        {
          id: bot.id,
          label: bot.label,
          status: bot.status.to_s,
          pair: pair_for(bot, base: base, quote: quote),
          quote_asset: quote,
          metrics: metrics,
          metrics_error: error
        }
      end

      def pair_for(bot, base:, quote:)
        if bot.dca_dual_asset?
          "#{bot.base0_asset&.symbol}+#{bot.base1_asset&.symbol}/#{quote}"
        elsif base && quote
          "#{base}/#{quote}"
        end
      end

      def safe_metrics(bot)
        metrics = bot.metrics
        return [nil, nil] if metrics.blank? || metrics[:pnl].nil?

        [{
          pnl: metrics[:pnl],
          invested: metrics[:total_quote_amount_invested]
        }, nil]
      rescue StandardError => e
        [nil, e.message]
      end
    end
  end
end
