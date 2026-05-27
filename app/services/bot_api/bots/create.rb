# frozen_string_literal: true

module BotApi
  module Bots
    # Creates and immediately starts a new DCA bot (single- or dual-asset).
    # The MCP tool used to inline this whole flow; pulling it here keeps the
    # decision tree in one place and gives REST callers a structured result.
    class Create
      VALID_INTERVALS = %w[hour day week month].freeze
      REQUIRED_PARAMS = %i[exchange_name base_asset quote_asset quote_amount interval].freeze

      def self.call(user:, **opts)
        new(user: user, **opts).call
      end

      # All keyword args are optional at the boundary so a malformed REST
      # request body cannot raise ArgumentError before we get a chance to
      # return a structured 422. Required fields are validated inside `call`.
      def initialize(user:, exchange_name: nil, base_asset: nil, quote_asset: nil,
                     quote_amount: nil, interval: nil,
                     second_base_asset: nil, allocation: nil, label: nil)
        @user = user
        @exchange_name = exchange_name
        @base_asset = base_asset
        @second_base_asset = second_base_asset
        @quote_asset = quote_asset
        @quote_amount = quote_amount
        @interval = interval
        @allocation = allocation
        @label = label
      end

      def call
        missing = REQUIRED_PARAMS.select { |k| instance_variable_get("@#{k}").blank? }
        return missing_required_parameter(missing) if missing.any?

        return invalid_interval unless VALID_INTERVALS.include?(@interval)

        exchange = Exchange.where('LOWER(name) = ?', @exchange_name.to_s.downcase).first
        return exchange_not_found unless exchange

        api_key = @user.api_keys.find_by(exchange: exchange, key_type: :trading, status: :correct)
        return api_key_missing(exchange) unless api_key

        first = find_pair(exchange, @base_asset, @quote_asset)
        return ticker_not_found(exchange, @base_asset) unless first

        if @second_base_asset.present?
          create_dual(exchange, first)
        else
          create_single(exchange, first)
        end
      end

      private

      def find_pair(exchange, base_symbol, quote_symbol)
        ticker = exchange.tickers.available
                         .joins(:base_asset, :quote_asset)
                         .where(assets: { symbol: base_symbol.to_s.upcase })
                         .where(quote_assets_tickers: { symbol: quote_symbol.to_s.upcase })
                         .first
        return nil unless ticker

        { base_asset_id: ticker.base_asset_id, quote_asset_id: ticker.quote_asset_id }
      end

      def create_single(exchange, asset_ids)
        bot = @user.bots.new(
          type: 'Bots::DcaSingleAsset',
          exchange: exchange,
          label: effective_label(@base_asset.upcase, @quote_asset.upcase, exchange),
          settings: {
            'base_asset_id' => asset_ids[:base_asset_id],
            'quote_asset_id' => asset_ids[:quote_asset_id],
            'quote_amount' => @quote_amount.to_f,
            'interval' => @interval
          }
        )
        save_and_start(bot)
      end

      def create_dual(exchange, first_asset_ids)
        second = find_pair(exchange, @second_base_asset, @quote_asset)
        return ticker_not_found(exchange, @second_base_asset) unless second
        return invalid_allocation if @allocation.present? && !@allocation.to_f.between?(0, 100)

        effective_allocation = @allocation.present? ? (@allocation.to_f / 100) : 0.5

        bot = @user.bots.new(
          type: 'Bots::DcaDualAsset',
          exchange: exchange,
          label: effective_label("#{@base_asset.upcase}+#{@second_base_asset.upcase}", @quote_asset.upcase, exchange),
          settings: {
            'base0_asset_id' => first_asset_ids[:base_asset_id],
            'base1_asset_id' => second[:base_asset_id],
            'quote_asset_id' => first_asset_ids[:quote_asset_id],
            'quote_amount' => @quote_amount.to_f,
            'interval' => @interval,
            'allocation0' => effective_allocation
          }
        )
        save_and_start(bot)
      end

      def save_and_start(bot)
        bot.set_missed_quote_amount
        return Result.failure(:validation_failed, 'bot_invalid', "Failed to create bot: #{bot.errors.full_messages.join(', ')}") unless bot.valid?

        if bot.save && bot.start(start_fresh: true)
          Result.success(serialize(bot), status: :created)
        else
          Result.failure(:validation_failed, 'bot_save_failed',
                         "Failed to create bot: #{bot.errors.full_messages.join(', ')}")
        end
      end

      def serialize(bot)
        {
          id: bot.id,
          label: bot.label,
          type: bot.type,
          status: bot.status.to_s,
          exchange: bot.exchange&.name,
          pair: pair_label(bot),
          quote_asset: @quote_asset.upcase,
          quote_amount: bot.settings['quote_amount'],
          interval: bot.settings['interval']
        }
      end

      def pair_label(bot)
        if bot.dca_dual_asset?
          "#{@base_asset.upcase}+#{@second_base_asset.upcase}/#{@quote_asset.upcase}"
        else
          "#{@base_asset.upcase}/#{@quote_asset.upcase}"
        end
      end

      def effective_label(pair_str, quote_str, exchange)
        return @label if @label.present?

        "#{pair_str}/#{quote_str} #{exchange.name}"
      end

      def missing_required_parameter(missing)
        Result.failure(:validation_failed, 'missing_required_parameter',
                       "Missing required parameter(s): #{missing.join(', ')}.")
      end

      def invalid_interval
        Result.failure(:validation_failed, 'invalid_interval',
                       "Invalid interval '#{@interval}'. Must be one of: #{VALID_INTERVALS.join(', ')}")
      end

      def invalid_allocation
        Result.failure(:validation_failed, 'invalid_allocation',
                       "Invalid allocation '#{@allocation}'. Must be a percentage between 0 and 100.")
      end

      def exchange_not_found
        available = Exchange.where(available: true).pluck(:name).join(', ')
        Result.failure(:not_found, 'exchange_not_found',
                       "Exchange '#{@exchange_name}' not found. Available: #{available}")
      end

      def api_key_missing(exchange)
        Result.failure(:permission_denied, 'api_key_missing',
                       "No valid API key found for #{exchange.name}. Please add an API key in Settings.")
      end

      def ticker_not_found(exchange, symbol)
        Result.failure(:not_found, 'pair_not_found',
                       "Trading pair #{symbol.to_s.upcase}/#{@quote_asset.upcase} not found on #{exchange.name}.")
      end
    end
  end
end
