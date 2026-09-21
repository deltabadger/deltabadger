# frozen_string_literal: true

module BotApi
  module Orders
    # A market order placed THROUGH a bot. An order sent without a bot goes straight to the venue
    # and belongs to nothing in the app; one that names a signal bot is that bot's order — a row on
    # its page, confirmed, charted and taxed like any other. The caller sizes it; the bot says which
    # pair, records it and classifies what went wrong (Bots::Signal::OrderSetter#execute_api_order).
    #
    # This is the door, so it is also where the bot may refuse: a stopped bot takes no orders, which
    # makes the stop button in the interface the kill switch for whatever is calling.
    class BotOrder
      AMOUNT_TYPES = %w[quote base].freeze
      # Matches the accountless services: a buy spends quote, a sell sells base.
      DEFAULT_AMOUNT_TYPE = { buy: 'quote', sell: 'base' }.freeze

      def self.call(user:, **opts)
        new(user: user, **opts).call
      end

      def initialize(user:, bot_id:, side:, amount: nil, amount_type: nil, exchange_name: nil,
                     base_asset: nil, quote_asset: nil, dry_run: false)
        @user = user
        @bot_id = bot_id
        @side = side
        @amount = amount
        @amount_type = amount_type.presence || DEFAULT_AMOUNT_TYPE.fetch(side)
        @exchange_name = exchange_name
        @base_asset = base_asset
        @quote_asset = quote_asset
        @dry_run = dry_run
      end

      def call
        amount = Number.parse(@amount)
        return failure(:validation_failed, 'invalid_number', 'amount must be a number greater than 0.') unless amount&.positive?

        return failure(:validation_failed, 'invalid_amount_type', "amount_type must be 'quote' or 'base'.") unless AMOUNT_TYPES.include?(@amount_type)

        # Strict: `"12abc".to_i` is 12 and `12.9.to_i` is 12, and either would trade through a bot
        # the caller never named. MCP sends a whole number as a Float (12.0), which this accepts.
        bot_id = Number.integer(@bot_id)
        return failure(:validation_failed, 'invalid_number', 'bot_id must be a whole number.') unless bot_id&.positive?

        bot = @user.bots.not_deleted.find_by(id: bot_id)
        refusal = bot_refusal(bot) || venue_refusal(bot)
        return refusal if refusal

        return success(bot, nil) if @dry_run

        refusal = market_refusal(bot)
        return refusal if refusal

        answer(bot, bot.execute_api_order(side: @side, amount: amount, amount_type: @amount_type.to_sym))
      end

      private

      def bot_refusal(bot)
        return failure(:not_found, 'bot_not_found', 'Bot not found.') unless bot

        unless bot.signal?
          return failure(:validation_failed, 'bot_not_orderable',
                         "Bot #{bot.id} is not a signal bot. Only a signal bot takes orders; every other type places its own.")
        end
        return failure(:conflict, 'bot_not_running', "Bot #{bot.id} is not running. Start it to place orders through it.") unless bot.working?
        return failure(:not_found, 'pair_not_found', "Bot #{bot.id} has no tradable pair on its exchange.") unless bot.ticker

        pair_mismatch(bot) || Lookup.untradable_refusal(bot.exchange, bot.ticker)
      end

      # The pair is the bot's. It may be left out; sent, it must agree — a caller that says ETH to a
      # BTC bot must not end up holding BTC.
      def pair_mismatch(bot)
        given = { exchange_name: [@exchange_name, bot.exchange.name], base_asset: [@base_asset, base(bot)],
                  quote_asset: [@quote_asset, quote(bot)] }
        wrong = given.select { |_, (sent, actual)| sent.present? && !sent.to_s.casecmp?(actual.to_s) }.keys
        return nil if wrong.empty?

        failure(:validation_failed, 'bot_pair_mismatch',
                "Bot #{bot.id} trades #{pair(bot)} on #{bot.exchange.name}; " \
                "#{wrong.join(', ')} say otherwise. Leave them out to use the bot's pair.")
      end

      def venue_refusal(bot)
        refusal = Lookup.wash_sale_refusal(@user, bot.ticker) if @side == :buy
        return refusal if refusal

        # The stored key, not bot.api_key: under a global dry run that one is a synthetic, always
        # correct key. Pending is asked first — Lookup.find_api_key only finds a :correct key, so a
        # key registered but not yet activated would otherwise read as missing, and it must never
        # reach a live venue call.
        key = @user.api_keys.find_by(exchange: bot.exchange, key_type: :trading)
        return failure(:conflict, 'api_key_pending', "The API key for #{bot.exchange.name} is still pending activation.") if key&.pending_activation?

        Lookup.api_key_missing(bot.exchange) unless key&.correct?
      end

      # Both can raise, and neither has sent anything to the venue.
      def market_refusal(bot)
        bot.ensure_exchange_authenticated
        return nil if bot.exchange.market_open?(tickers: [bot.ticker])

        failure(:conflict, 'market_closed', "The market for #{base(bot)} on #{bot.exchange.name} is closed.")
      rescue StandardError => e
        failure(:upstream_failed, 'exchange_unavailable', "Could not reach #{bot.exchange.name}: #{e.message}")
      end

      def answer(bot, outcome)
        errors = outcome.errors.to_sentence
        case outcome.status
        when :submitted then success(bot, outcome)
        when :skipped
          failure(:validation_failed, 'below_minimum_amount',
                  "The order is below #{bot.exchange.name}'s minimum for #{pair(bot)}.")
        when :locked
          failure(:conflict, 'wash_sale_locked', "#{base(bot)} is locked after a sale at a loss.")
        when :ambiguous
          failure(:upstream_failed, 'placement_ambiguous',
                  "The order may or may not have been placed: #{errors}. Check #{bot.exchange.name} before sending it again.")
        else
          failure(:upstream_failed, 'order_failed', "Order failed: #{errors}")
        end
      end

      # `outcome` is nil on a dry run: nothing was placed, so there is nothing to point at.
      def success(bot, outcome)
        Result.success({
                         dry_run: @dry_run, bot_id: bot.id,
                         transaction_id: outcome&.transaction&.id, order_id: outcome&.order_id,
                         exchange: bot.exchange.name, pair: pair(bot),
                         side: @side.to_s, order_type: 'market', amount: @amount, amount_type: @amount_type
                       }, status: :created)
      end

      # Asset symbols, the API's one vocabulary — creating the bot, reading it and its transaction
      # rows all use them. The ticker's own base/quote are the exchange's aliases (Kraken's XBT).
      def base(bot) = bot.base_asset.symbol
      def quote(bot) = bot.quote_asset.symbol
      def pair(bot) = "#{base(bot)}/#{quote(bot)}"

      def failure(status, code, message) = Result.failure(status, code, message)
    end
  end
end
