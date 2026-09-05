# frozen_string_literal: true

module BotApi
  module Bots
    # Creates and immediately starts a new DCA bot (single- or dual-asset).
    # The MCP tool used to inline this whole flow; pulling it here keeps the
    # decision tree in one place and gives REST callers a structured result.
    class Create
      include CreateSupport

      # Kept as an alias so callers and tests that reach for it through Create still resolve.
      VALID_INTERVALS = CreateSupport::VALID_INTERVALS
      REQUIRED_PARAMS = %i[exchange_name base_asset quote_asset quote_amount interval].freeze

      def self.call(user:, **opts)
        new(user: user, **opts).call
      end

      # All keyword args are optional at the boundary so a malformed REST
      # request body cannot raise ArgumentError before we get a chance to
      # return a structured 422. Required fields are validated inside `call`.
      def initialize(user:, exchange_name: nil, base_asset: nil, quote_asset: nil,
                     quote_amount: nil, interval: nil,
                     second_base_asset: nil, allocation: nil, label: nil, start_at: nil)
        @user = user
        @exchange_name = exchange_name
        @base_asset = base_asset
        @second_base_asset = second_base_asset
        @quote_asset = quote_asset
        @quote_amount = quote_amount
        @interval = interval
        @allocation = allocation
        @label = label
        @start_at = start_at
      end

      def call
        err = missing_required(REQUIRED_PARAMS)
        return err if err
        return invalid_interval unless VALID_INTERVALS.include?(@interval)

        exchange = find_exchange(@exchange_name)
        return exchange_not_found unless exchange
        return api_key_missing(exchange) unless trading_key(exchange)

        first = find_pair(exchange, @base_asset, @quote_asset)
        return ticker_not_found(exchange, @base_asset) unless first

        if @second_base_asset.present?
          create_basket(exchange, first)
        else
          create_single(exchange, first)
        end
      end

      private

      def create_single(exchange, asset_ids)
        bot = @user.bots.new(
          type: 'Bots::DcaSingleAsset',
          exchange: exchange,
          label: @label,
          settings: {
            'base_asset_id' => asset_ids[:base_asset_id],
            'quote_asset_id' => asset_ids[:quote_asset_id],
            'quote_amount' => @quote_amount.to_f,
            'interval' => @interval
          }
        )
        save_and_start(bot)
      end

      # Two assets is a basket. The parameter is still named second_base_asset for compatibility;
      # the bot behind it is now the general composition type, the same one the wizard builds.
      def create_basket(exchange, first_asset_ids)
        second = find_pair(exchange, @second_base_asset, @quote_asset)
        return ticker_not_found(exchange, @second_base_asset) unless second
        return invalid_allocation if @allocation.present? && !@allocation.to_f.between?(0, 100)

        effective_allocation = @allocation.present? ? (@allocation.to_f / 100) : 0.5

        bot = @user.bots.new(
          type: 'Bots::DcaMultiAsset',
          exchange: exchange,
          label: @label,
          settings: {
            'quote_asset_id' => first_asset_ids[:quote_asset_id],
            'quote_amount' => @quote_amount.to_f,
            'interval' => @interval,
            'weighting' => 'manual',
            'allocations' => {
              first_asset_ids[:base_asset_id].to_s => effective_allocation,
              second[:base_asset_id].to_s => 1 - effective_allocation
            }
          }
        )
        save_and_start(bot)
      end

      def serialize(bot)
        {
          id: bot.id,
          label: bot.label,
          type: bot.type,
          status: bot.status.to_s,
          exchange: bot.exchange&.name,
          pair: pair_label,
          quote_asset: @quote_asset.upcase,
          quote_amount: bot.settings['quote_amount'],
          interval: bot.settings['interval'],
          started_at: bot.started_at&.iso8601
        }
      end

      # Keyed off the request, not the bot's type: a basket and a single-asset bot no longer differ
      # by class in a way this can read, and falling through would label a two-asset bot as one.
      def pair_label
        if @second_base_asset.present?
          "#{@base_asset.upcase}+#{@second_base_asset.upcase}/#{@quote_asset.upcase}"
        else
          "#{@base_asset.upcase}/#{@quote_asset.upcase}"
        end
      end

      def invalid_allocation
        Result.failure(:validation_failed, 'invalid_allocation',
                       "Invalid allocation '#{@allocation}'. Must be a percentage between 0 and 100.")
      end
    end
  end
end
