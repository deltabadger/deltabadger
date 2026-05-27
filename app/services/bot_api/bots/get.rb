# frozen_string_literal: true

module BotApi
  module Bots
    # Returns the detail view for a single bot, including computed metrics.
    # Metrics errors don't fail the call — they're carried on the result so
    # the MCP presenter can show "Metrics unavailable: ..." while REST clients
    # see `metrics: nil, metrics_error: "..."`.
    class Get
      def self.call(user:, bot_id:)
        new(user: user, bot_id: bot_id).call
      end

      def initialize(user:, bot_id:)
        @user = user
        @bot_id = bot_id
      end

      def call
        bot = @user.bots.not_deleted.find_by(id: @bot_id.to_i)
        return Result.failure(:not_found, 'bot_not_found', 'Bot not found.') unless bot

        Result.success(detail_for(bot))
      end

      private

      def detail_for(bot)
        quote = bot.quote_asset&.symbol
        base = bot.respond_to?(:base_asset) ? bot.base_asset&.symbol : nil
        metrics, metrics_error = safe_metrics(bot)

        {
          id: bot.id,
          label: bot.label,
          type: bot.type,
          status: bot.status.to_s,
          exchange: bot.exchange&.name,
          pair: pair_for(bot, base: base, quote: quote),
          base_asset: base,
          quote_asset: quote,
          interval: bot.settings['interval'],
          quote_amount: bot.settings['quote_amount'],
          orders_executed: bot.successful_transaction_count,
          started_at: bot.started_at,
          metrics: metrics,
          metrics_error: metrics_error
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
        return [nil, nil] if metrics.blank?

        [{
          invested: metrics[:total_quote_amount_invested],
          value: metrics[:total_amount_value_in_quote],
          pnl: metrics[:pnl],
          average_buy_price: metrics[:average_buy_price],
          total_base_amount: metrics[:total_base_amount]
        }, nil]
      rescue StandardError => e
        [nil, e.message]
      end
    end
  end
end
