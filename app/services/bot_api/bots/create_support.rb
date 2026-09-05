# frozen_string_literal: true

module BotApi
  module Bots
    # The parts every creator shares: parameter presence, venue and key lookup, pair lookup,
    # and the validate-then-start step. Including classes define `serialize(bot)`.
    module CreateSupport
      VALID_INTERVALS = %w[hour day week month].freeze

      private

      def missing_required(keys)
        missing = keys.select { |k| instance_variable_get("@#{k}").blank? }
        return nil if missing.empty?

        Result.failure(:validation_failed, 'missing_required_parameter',
                       "Missing required parameter(s): #{missing.join(', ')}.")
      end

      # tradeable = available and not retired: exactly the set the wizard's exchange picker
      # offers. A name lookup alone would reach a venue the picker refuses.
      def find_exchange(name)
        Exchange.tradeable.where('LOWER(name) = ?', name.to_s.downcase).first
      end

      # Strict, or a typo becomes a zero-sized bot that fails validation with a message about
      # a number nobody sent.
      def positive_number(value, name)
        number = Number.parse(value)
        return number if number&.positive?

        Result.failure(:validation_failed, 'invalid_number', "#{name} must be a number greater than 0.")
      end

      def trading_key(exchange)
        @user.api_keys.find_by(exchange: exchange, key_type: :trading, status: :correct)
      end

      def find_pair(exchange, base_symbol, quote_symbol)
        ticker = exchange.tickers.available
                         .joins(:base_asset, :quote_asset)
                         .where(assets: { symbol: base_symbol.to_s.upcase })
                         .where(quote_assets_tickers: { symbol: quote_symbol.to_s.upcase })
                         .first
        return nil unless ticker

        { base_asset_id: ticker.base_asset_id, quote_asset_id: ticker.quote_asset_id }
      end

      # nil start_at means "now" (the documented default); a given-but-blank one must fail
      # validation, never fall through to an unintended immediate buy. `bot.start` performs the
      # insert, so a validation failure leaves no orphaned bot.
      def save_and_start(bot)
        bot.schedule_start_at(@start_at) if !@start_at.nil? && bot.respond_to?(:schedule_start_at)
        bot.set_missed_quote_amount if bot.respond_to?(:set_missed_quote_amount)
        unless bot.valid?(:start)
          return Result.failure(:validation_failed, 'bot_invalid',
                                "Failed to create bot: #{bot.errors.full_messages.join(', ')}")
        end

        if bot.start(start_fresh: true)
          Result.success(serialize(bot), status: :created)
        else
          Result.failure(:validation_failed, 'bot_save_failed',
                         "Failed to create bot: #{bot.errors.full_messages.join(', ')}")
        end
      end

      def invalid_interval
        Result.failure(:validation_failed, 'invalid_interval',
                       "Invalid interval '#{@interval}'. Must be one of: #{VALID_INTERVALS.join(', ')}")
      end

      def exchange_not_found
        Result.failure(:not_found, 'exchange_not_found',
                       "Exchange '#{@exchange_name}' not found. Available: #{Exchange.tradeable.pluck(:name).join(', ')}")
      end

      def api_key_missing(exchange)
        Result.failure(:permission_denied, 'api_key_missing',
                       "No valid API key found for #{exchange.name}. Please add an API key in Settings.")
      end

      def ticker_not_found(exchange, symbol)
        Result.failure(:not_found, 'pair_not_found',
                       "Trading pair #{symbol.to_s.upcase}/#{@quote_asset.to_s.upcase} not found on #{exchange.name}.")
      end
    end
  end
end
