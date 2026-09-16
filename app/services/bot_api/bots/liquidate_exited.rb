# frozen_string_literal: true

module BotApi
  module Bots
    # Sells holdings of a composition bot — current members, or ones that left the composition.
    # Deliberately manual and explicit: each is a taxable disposal (see
    # Bot::Composition::Liquidatable). Every symbol must be one the bot itself reports as held —
    # something it does not hold is refused before any price is read, so a cold metrics cache cannot
    # turn a refusal into a 500.
    #
    # `symbol` is a String or an Array: it is the REST body's name and the MCP tool's property, so it
    # keeps that name. The page sends the list its confirmation displayed.
    class LiquidateExited
      def self.call(user:, bot_id:, symbol:, dry_run: false)
        new(user: user, bot_id: bot_id, symbol: symbol, dry_run: dry_run).call
      end

      def initialize(user:, bot_id:, symbol:, dry_run: false)
        @user = user
        @bot_id = bot_id
        @symbols = Array(symbol).map { |value| value.to_s.upcase }.uniq
        @dry_run = dry_run
      end

      def call
        bot = @user.bots.not_deleted.find_by(id: @bot_id.to_i)
        return Result.failure(:not_found, 'bot_not_found', 'Bot not found.') unless bot

        unless bot.respond_to?(:held_symbols)
          return Result.failure(:validation_failed, 'not_composition_bot',
                                'Only index and basket bots hold sellable positions.')
        end
        return Result.failure(:conflict, 'bot_archived', "Bot '#{bot.label}' is archived; reactivate it first.") if bot.archived?

        # All or nothing, unlike the web controller, which intersects: that path has a confirmation
        # in which to show the user a reduced list, and this one does not.
        missing = @symbols - bot.held_symbols
        if @symbols.empty? || missing.any?
          return Result.failure(:not_found, 'holding_not_held',
                                "#{missing.to_sentence.presence || 'No symbol'} is not a position this bot holds. " \
                                "Held: #{bot.held_symbols.join(', ').presence || 'none'}.")
        end
        return Result.failure(:conflict, 'market_closed', 'The market is closed; try again when it opens.') if market_closed?(bot)

        unless @dry_run
          # Marked HERE, in the request, rather than in the job: the job queues behind the exchange
          # semaphore for an unbounded wait, and the page has to say a sale is coming for all of it.
          # The token comes back so only this request's own job can clear it again.
          token = bot.mark_selling!
          # Repainted BEFORE the enqueue, not after. A job that ran and finished in between would
          # clear the marker and broadcast the idle tables, and this instance still holds the marker
          # in memory — so a repaint after the enqueue could put the spinners back over a sale that
          # is already done, with nothing left to take them down again.
          bot.broadcast_selling_state(cached_only: true)
          begin
            Bot::LiquidateExitedJob.perform_later(bot, symbols: @symbols, selling_token: token)
          rescue StandardError
            # The marker is cleared by the JOB, so an enqueue that never produced one leaves nothing
            # to take it down: a quarter of an hour of spinner for a sale that never started, which
            # is worse than the failure itself. Unwound here, then raised on as before.
            bot.clear_selling!(token)
            bot.broadcast_selling_state(cached_only: true)
            raise
          end
          bot.log_activity('liquidation_requested', level: :info,
                                                    details: { user_id: @user.id, base: @symbols.join(', ') })
        end
        # `symbol` stays, and a one-element join IS that element — so the REST envelope and the MCP
        # sentence are byte-identical for every caller that names one position. `symbols` is the
        # honest field for a batch.
        Result.success({ id: bot.id, label: bot.label, symbol: @symbols.join(', '), symbols: @symbols,
                         dry_run: @dry_run }, status: :accepted)
      end

      private

      # Authenticated first (Alpaca answers from /v2/clock), and failing OPEN on any other error:
      # a convenience check must never be the thing that stops a sale the job could have made.
      def market_closed?(bot)
        bot.ensure_exchange_authenticated
        !bot.exchange.market_open?(tickers: bot.liquidation_tickers(symbols: @symbols))
      rescue StandardError => e
        Rails.logger.warn("liquidation market check failed bot=#{bot.id}: #{e.message}")
        false
      end
    end
  end
end
