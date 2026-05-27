# frozen_string_literal: true

module BotApi
  module Bots
    # Returns the user's non-deleted bots, optionally filtered by status.
    # Pure service: no HTTP or text-presentation knowledge. The MCP tool
    # wraps this and formats text; REST controllers serialize the Hash.
    class List
      def self.call(user:, status: nil)
        new(user: user, status: status).call
      end

      def initialize(user:, status: nil)
        @user = user
        @status = status.presence
      end

      def call
        scope = @user.bots.not_deleted.includes(:exchange)
        scope = scope.where(status: @status) if @status

        rows = scope.map { |bot| row_for(bot) }
        Result.success({ count: rows.size, bots: rows })
      end

      private

      def row_for(bot)
        quote = bot.quote_asset&.symbol
        base = bot.respond_to?(:base_asset) ? bot.base_asset&.symbol : nil

        {
          id: bot.id,
          label: bot.label,
          type: bot.type,
          pair: pair_for(bot, base: base, quote: quote),
          base_asset: base,
          quote_asset: quote,
          exchange: bot.exchange&.name,
          status: bot.status.to_s,
          interval: bot.settings['interval'],
          quote_amount: bot.settings['quote_amount']
        }
      end

      def pair_for(bot, base:, quote:)
        if bot.dca_dual_asset?
          "#{bot.base0_asset&.symbol}+#{bot.base1_asset&.symbol}/#{quote}"
        elsif base && quote
          "#{base}/#{quote}"
        end
      end
    end
  end
end
