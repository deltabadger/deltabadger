# frozen_string_literal: true

module BotApi
  module Bots
    # Sells one holding the composition has dropped. Deliberately manual and explicit: it is a
    # taxable disposal (see Bot::Composition::Liquidatable). The symbol must be one the bot
    # itself reports as exited — a current member, or something it does not hold, is refused
    # before any price is read, so a cold metrics cache cannot turn a refusal into a 500.
    class LiquidateExited
      def self.call(user:, bot_id:, symbol:, dry_run: false)
        new(user: user, bot_id: bot_id, symbol: symbol, dry_run: dry_run).call
      end

      def initialize(user:, bot_id:, symbol:, dry_run: false)
        @user = user
        @bot_id = bot_id
        @symbol = symbol.to_s.upcase
        @dry_run = dry_run
      end

      def call
        bot = @user.bots.not_deleted.find_by(id: @bot_id.to_i)
        return Result.failure(:not_found, 'bot_not_found', 'Bot not found.') unless bot

        unless bot.respond_to?(:exited_symbols)
          return Result.failure(:validation_failed, 'not_composition_bot',
                                'Only index and basket bots have exited holdings.')
        end
        return Result.failure(:conflict, 'bot_archived', "Bot '#{bot.label}' is archived; reactivate it first.") if bot.archived?

        unless bot.exited_symbols.include?(@symbol)
          return Result.failure(:not_found, 'holding_not_exited',
                                "#{@symbol} is not a holding this bot's composition has dropped. " \
                                "Exited: #{bot.exited_symbols.join(', ').presence || 'none'}.")
        end
        return Result.failure(:conflict, 'market_closed', 'The market is closed; try again when it opens.') if market_closed?(bot)

        unless @dry_run
          Bot::LiquidateExitedJob.perform_later(bot, symbol: @symbol)
          bot.log_activity('liquidation_requested', level: :info, details: { user_id: @user.id, base: @symbol })
        end
        Result.success({ id: bot.id, label: bot.label, symbol: @symbol, dry_run: @dry_run }, status: :accepted)
      end

      private

      # Authenticated first (Alpaca answers from /v2/clock), and failing OPEN on any other error:
      # a convenience check must never be the thing that stops a sale the job could have made.
      def market_closed?(bot)
        bot.ensure_exchange_authenticated
        !bot.exchange.market_open?(tickers: bot.liquidation_tickers(symbol: @symbol))
      rescue StandardError => e
        Rails.logger.warn("liquidation market check failed bot=#{bot.id}: #{e.message}")
        false
      end
    end
  end
end
