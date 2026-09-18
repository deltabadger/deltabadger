# frozen_string_literal: true

module BotApi
  module Bots
    # Sells holdings of a composition bot — current members, or ones that left the composition.
    # Deliberately manual and explicit: each is a taxable disposal (see
    # Bot::Composition::Liquidatable). Every holding must be one the bot itself reports as held —
    # something it does not hold is refused before any price is read, so a cold metrics cache cannot
    # turn a refusal into a 500.
    #
    # `symbol` is a String or an Array: it is the REST body's name and the MCP tool's property, so it
    # keeps that name. Each entry is a holding's key (as get_bot_details lists it), an asset id, or a
    # symbol only one holding has. A symbol two holdings share is refused with their keys. The page
    # sends the keys its confirmation displayed, with the asset each named (`asset_id`).
    class LiquidateExited
      def self.call(user:, bot_id:, symbol:, asset_id: nil, dry_run: false)
        new(user: user, bot_id: bot_id, symbol: symbol, asset_id: asset_id, dry_run: dry_run).call
      end

      def initialize(user:, bot_id:, symbol:, asset_id: nil, dry_run: false)
        @user = user
        @bot_id = bot_id
        @identifiers = Array(symbol).map { |value| value.to_s.strip }.uniq
        @expected = Array(asset_id).map(&:to_s)
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
        held = bot.held_assets
        @holdings = []
        @identifiers.each_with_index do |identifier, index|
          matches = held_matching(held, identifier)
          if matches.size > 1
            return Result.failure(:validation_failed, 'holding_ambiguous',
                                  "#{identifier} names more than one holding: #{matches.keys.join(', ')}. Use one of those.")
          end
          # The asset the page showed beside this key: a key now naming another asset is not held.
          matches = {} if @expected[index].present? && matches.values.first.to_s != @expected[index]
          if matches.empty?
            return Result.failure(:not_found, 'holding_not_held',
                                  "#{identifier} is not a position this bot holds. " \
                                  "Held: #{held.keys.join(', ').presence || 'none'}.")
          end
          @holdings << matches.first
        end
        return Result.failure(:not_found, 'holding_not_held', 'No symbol is a position this bot holds.') if @holdings.empty?

        @holdings.uniq!
        @symbols = @holdings.map(&:first)
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
            Bot::LiquidateExitedJob.perform_later(bot, holdings: @holdings, selling_token: token)
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

      # The held keys an identifier names: the key itself, the holding of that asset id, or every holding
      # whose asset has that symbol (case-insensitive).
      def held_matching(held, identifier)
        return held.slice(identifier) if held.key?(identifier)
        return held.select { |_key, asset_id| asset_id.to_s == identifier } if identifier.match?(/\A\d+\z/)

        symbols = Asset.where(id: held.values).pluck(:id, :symbol).to_h
        held.select { |_key, asset_id| symbols[asset_id]&.casecmp?(identifier) }
      end

      # Authenticated first (Alpaca answers from /v2/clock), and failing OPEN on any other error:
      # a convenience check must never be the thing that stops a sale the job could have made.
      def market_closed?(bot)
        bot.ensure_exchange_authenticated
        !bot.exchange.market_open?(tickers: bot.liquidation_tickers(holdings: @holdings))
      rescue StandardError => e
        Rails.logger.warn("liquidation market check failed bot=#{bot.id}: #{e.message}")
        false
      end
    end
  end
end
