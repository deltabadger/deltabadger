# frozen_string_literal: true

module BotApi
  module Bots
    # Creates and immediately starts a new DCA bot: one base asset, or a basket of 2-20.
    # The MCP tool used to inline this whole flow; pulling it here keeps the
    # decision tree in one place and gives REST callers a structured result.
    #
    # `::Bots::` throughout: inside BotApi::Bots a bare `Bots::DcaMultiAsset` would resolve to
    # BotApi::Bots::DcaMultiAsset and raise.
    class Create
      include CreateSupport

      # Kept as an alias so callers and tests that reach for it through Create still resolve.
      VALID_INTERVALS = CreateSupport::VALID_INTERVALS
      # base_asset is validated inside `call`: a basket supplies `assets` instead.
      REQUIRED_PARAMS = %i[exchange_name quote_asset quote_amount interval].freeze
      WEIGHTINGS = ::Bots::DcaMultiAsset::WEIGHTINGS

      def self.call(user:, **opts)
        new(user: user, **opts).call
      end

      # All keyword args are optional at the boundary so a malformed REST
      # request body cannot raise ArgumentError before we get a chance to
      # return a structured 422. Required fields are validated inside `call`.
      def initialize(user:, exchange_name: nil, base_asset: nil, quote_asset: nil,
                     quote_amount: nil, interval: nil,
                     second_base_asset: nil, allocation: nil, assets: nil, weighting: nil,
                     label: nil, start_at: nil)
        @user = user
        @exchange_name = exchange_name
        @base_asset = base_asset
        @second_base_asset = second_base_asset
        @quote_asset = quote_asset
        @quote_amount = quote_amount
        @interval = interval
        @allocation = allocation
        @assets = assets
        @weighting = weighting.presence || 'manual'
        @label = label
        @start_at = start_at
      end

      def call
        # base_asset is required only when no basket was asked for, so an empty body still
        # enumerates all five fields a single-asset create needs.
        err = missing_required(basket_requested? ? REQUIRED_PARAMS : REQUIRED_PARAMS + %i[base_asset])
        return err if err
        return invalid_interval unless VALID_INTERVALS.include?(@interval)
        return invalid_weighting unless WEIGHTINGS.include?(@weighting)
        return invalid_allocation if legacy_allocation_invalid?

        exchange = find_exchange(@exchange_name)
        return exchange_not_found unless exchange
        return api_key_missing(exchange) unless trading_key(exchange)

        @amount = positive_number(@quote_amount, 'quote_amount')
        return @amount if @amount.is_a?(Result)

        basket = basket_entries
        return invalid_basket if basket.nil?

        basket.empty? ? create_single(exchange) : create_basket(exchange, basket)
      end

      private

      def basket_requested? = @assets.present? || @second_base_asset.present?

      # The legacy two-asset pair keeps its own refusal: a caller who sent `allocation` should not
      # be told about `assets`, a parameter it never used.
      def legacy_allocation_invalid?
        @second_base_asset.present? && @allocation.present? && Number.within(@allocation, 0.0..100.0).nil?
      end

      # [{symbol:, allocation:}] from whichever shape arrived; [] means single-asset; nil means
      # unparseable, malformed, duplicated, or out of bounds — every one of those is a 422.
      def basket_entries
        entries =
          if @assets.present?
            @assets.is_a?(String) ? @assets.split(',').map { |e| parse_asset(e) } : Array(@assets).map { |e| normalize_asset(e) }
          elsif @second_base_asset.present?
            first = @allocation.present? ? Number.within(@allocation, 0.0..100.0) : 50.0
            return nil if first.nil?

            [{ symbol: @base_asset, allocation: first }, { symbol: @second_base_asset, allocation: 100 - first }]
          else
            []
          end
        return entries if entries.empty?
        return nil if entries.any?(&:nil?)
        return nil unless entries.size.between?(::Bots::DcaMultiAsset::MIN_ASSETS, ::Bots::DcaMultiAsset::MAX_ASSETS)
        return nil if entries.map { |e| e[:symbol] }.uniq.size != entries.size

        entries
      end

      # 'BTC' gives no weight; 'BTC:' gives one and gets it wrong. Only the first may fall through
      # to the equal split — the second must be refused, or a typo silently reweights the basket.
      def parse_asset(entry)
        symbol, allocation = entry.to_s.strip.split(':', 2)
        return normalize_asset(symbol: symbol) if allocation.nil?

        normalize_asset(symbol: symbol, allocation: allocation)
      end

      # A REST body can put anything in the array — a number, a string, a nested list. Only a
      # hash-shaped entry is an asset; the rest is refused before to_h can raise.
      def normalize_asset(entry)
        return nil unless entry.is_a?(Hash) || entry.is_a?(ActionController::Parameters)

        attributes = entry.to_h.with_indifferent_access
        symbol = attributes[:symbol].to_s.strip.upcase
        return nil if symbol.blank?
        # Absent, not blank: a key that is present carries a weight the caller meant.
        return { symbol: symbol, allocation: nil } unless attributes.key?(:allocation)

        allocation = Number.within(attributes[:allocation], 0.0..100.0)
        return nil if allocation.nil?

        { symbol: symbol, allocation: allocation }
      end

      def create_single(exchange)
        first = find_pair(exchange, @base_asset, @quote_asset)
        return ticker_not_found(exchange, @base_asset) unless first

        bot = @user.bots.new(
          type: 'Bots::DcaSingleAsset',
          exchange: exchange,
          label: @label,
          settings: {
            'base_asset_id' => first[:base_asset_id],
            'quote_asset_id' => first[:quote_asset_id],
            'quote_amount' => @amount.to_f,
            'interval' => @interval
          }
        )
        save_and_start(bot)
      end

      def create_basket(exchange, entries)
        pairs = entries.map { |entry| [entry, find_pair(exchange, entry[:symbol], @quote_asset)] }
        if (missing = pairs.find { |_, pair| pair.nil? })
          return ticker_not_found(exchange, missing.first[:symbol])
        end

        weights = basket_weights(entries)
        return allocations_unbalanced unless weights

        allocations = pairs.each_with_index.to_h { |(_, pair), i| [pair[:base_asset_id].to_s, weights[i]] }

        bot = @user.bots.new(
          type: 'Bots::DcaMultiAsset',
          exchange: exchange,
          label: @label,
          settings: {
            'quote_asset_id' => pairs.first.last[:quote_asset_id],
            'quote_amount' => @amount.to_f,
            'interval' => @interval,
            'weighting' => @weighting,
            'allocations' => allocations
          }
        )
        # The model keeps stored weights when any member has no market cap (stocks and ETFs never
        # do), so a market_cap basket that cannot be sized would trade the equal split while
        # reporting market_cap. Refuse it instead of pretending.
        return market_cap_unavailable(entries) if @weighting == 'market_cap' && !bot.market_cap_weightable?

        @basket_symbols = entries.map { |entry| entry[:symbol] }
        save_and_start(bot)
      end

      # Fractions summing to 1: equal when no weight was given (or the bot derives them), else the
      # given percentages, which must all be present and add up to 100.
      def basket_weights(entries)
        given = entries.map { |entry| entry[:allocation] }
        return Array.new(entries.size, 1.0 / entries.size) if @weighting == 'market_cap' || given.none?
        return nil unless given.all? && (given.sum - 100).abs <= 0.1

        # Floats at the model boundary: the composition stores float weights.
        given.map { |pct| (pct / 100).to_f }
      end

      def serialize(bot)
        {
          id: bot.id,
          label: bot.label,
          type: bot.type,
          status: bot.status.to_s,
          exchange: bot.exchange&.name,
          pair: pair_label,
          quote_asset: @quote_asset.to_s.upcase,
          quote_amount: bot.settings['quote_amount'],
          interval: bot.settings['interval'],
          started_at: bot.started_at&.iso8601
        }
      end

      # Keyed off the request, not the bot's type: a basket and a single-asset bot no longer differ
      # by class in a way this can read, and falling through would label a two-asset bot as one.
      def pair_label
        symbols = @basket_symbols || [@base_asset.to_s.upcase]
        "#{symbols.join('+')}/#{@quote_asset.to_s.upcase}"
      end

      def invalid_allocation
        Result.failure(:validation_failed, 'invalid_allocation',
                       "Invalid allocation '#{@allocation}'. Must be a percentage between 0 and 100.")
      end

      def invalid_basket
        Result.failure(:validation_failed, 'invalid_basket',
                       "assets must list #{::Bots::DcaMultiAsset::MIN_ASSETS} to #{::Bots::DcaMultiAsset::MAX_ASSETS} " \
                       "symbols, e.g. 'BTC:60,ETH:40' or 'BTC,ETH'.")
      end

      def invalid_weighting
        Result.failure(:validation_failed, 'invalid_weighting', "weighting must be one of: #{WEIGHTINGS.join(', ')}")
      end

      def market_cap_unavailable(entries)
        Result.failure(:validation_failed, 'market_cap_unavailable',
                       'Market-cap weighting needs a market cap for every asset; not every one of ' \
                       "#{entries.map { |entry| entry[:symbol] }.join(', ')} has one. Give weights instead.")
      end

      def allocations_unbalanced
        Result.failure(:validation_failed, 'allocations_unbalanced',
                       'Either give every asset a weight summing to 100, or none.')
      end
    end
  end
end
