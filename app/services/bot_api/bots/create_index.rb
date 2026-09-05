# frozen_string_literal: true

module BotApi
  module Bots
    # `::Bots::` throughout: inside BotApi::Bots a bare `Bots::DcaIndex` resolves to
    # BotApi::Bots::DcaIndex and raises, exactly as `Tax::` does under BotApi::Tax.
    class CreateIndex
      include CreateSupport

      REQUIRED = %i[exchange_name quote_asset quote_amount interval].freeze

      def self.call(user:, **opts)
        new(user: user, **opts).call
      end

      def initialize(user:, exchange_name: nil, quote_asset: nil, quote_amount: nil, interval: nil,
                     index: nil, num_coins: nil, allocation_flattening: nil, label: nil, start_at: nil)
        @user = user
        @exchange_name = exchange_name
        @quote_asset = quote_asset
        @quote_amount = quote_amount
        @interval = interval
        @index = index.presence || Index::TOP_COINS_EXTERNAL_ID
        @num_coins = num_coins
        @allocation_flattening = allocation_flattening
        @label = label
        @start_at = start_at
      end

      def call
        err = missing_required(REQUIRED)
        return err if err
        return invalid_interval unless VALID_INTERVALS.include?(@interval)

        unless MarketData.configured?
          return Result.failure(:validation_failed, 'market_data_not_configured',
                                'Market data provider is not configured. ' \
                                'Set up CoinGecko or Deltabadger market data in Settings first.')
        end

        exchange = find_exchange(@exchange_name)
        return exchange_not_found unless exchange
        return api_key_missing(exchange) unless trading_key(exchange)

        # Scoped to the venue: an index the picker would not offer here must not be creatable here.
        index = visible_indices.available_on_exchange(exchange).find_by(external_id: @index)
        unless index
          return Result.failure(:not_found, 'index_not_found',
                                "Index '#{@index}' not found or not available on #{exchange.name}. " \
                                "Use 'list_indices' with exchange_name to see what is.")
        end

        quote_amount = positive_number(@quote_amount, 'quote_amount')
        return quote_amount if quote_amount.is_a?(Result)

        flattening = @allocation_flattening.present? ? Number.within(@allocation_flattening, 0.0..1.0) : 0.0
        if flattening.nil?
          return Result.failure(:validation_failed, 'invalid_number',
                                'allocation_flattening must be a number between 0 and 1.')
        end

        num_coins = @num_coins.present? ? Number.integer(@num_coins) : nil
        return Result.failure(:validation_failed, 'invalid_number', 'num_coins must be a whole number.') if @num_coins.present? && num_coins.nil?

        # The quote is checked the way the wizard's quote picker decides what to offer: through
        # the model's own query, which for a category index counts only that index's members and
        # needs MINIMUM_SUPPORTED_COINS of them trading against the quote on this venue.
        # Floats at the model boundary: DcaIndex stores floats, and these are validated numbers now.
        bot = @user.bots.new(type: 'Bots::DcaIndex', exchange: exchange, label: @label,
                             settings: { 'interval' => @interval,
                                         'allocation_flattening' => flattening.to_f }.merge(index_settings(index)))
        quote = bot.available_assets_for_current_settings(asset_type: :quote_asset).find_by(symbol: @quote_asset.to_s.upcase)
        unless quote
          return Result.failure(:not_found, 'quote_asset_not_found',
                                "Fewer than #{Index::ExchangeAvailability::MINIMUM_SUPPORTED_COINS} members of " \
                                "'#{@index}' trade against #{@quote_asset.to_s.upcase} on #{exchange.name}. " \
                                'Try another quote asset or exchange.')
        end

        bot.quote_asset_id = quote.id
        bot.quote_amount = quote_amount.to_f
        bot.num_coins = num_coins if num_coins
        save_and_start(bot)
      end

      private

      def visible_indices
        MarketDataSettings.deltabadger? ? Index.all : Index.where.not(source: Index::SOURCE_DELTABADGER)
      end

      # Exactly what Bots::DcaIndexes::PickIndicesController#create writes into the wizard session.
      def index_settings(index)
        if index.external_id == Index::TOP_COINS_EXTERNAL_ID
          { 'index_type' => ::Bots::DcaIndex::INDEX_TYPE_TOP, 'index_category_id' => nil,
            'index_name' => nil, 'index_name_prefix' => nil }
        else
          { 'index_type' => ::Bots::DcaIndex::INDEX_TYPE_CATEGORY, 'index_category_id' => index.external_id,
            'index_name' => index.name,
            'index_name_prefix' => ::Bots::DcaIndex::COUNT_NAMED_INDEX_PREFIXES[index.external_id] }
        end
      end

      def serialize(bot)
        {
          id: bot.id, label: bot.label, type: bot.type, status: bot.status.to_s, exchange: bot.exchange&.name,
          index: @index, num_coins: bot.num_coins, allocation_flattening: bot.allocation_flattening,
          quote_asset: @quote_asset.to_s.upcase, quote_amount: bot.settings['quote_amount'],
          interval: bot.settings['interval'], started_at: bot.started_at&.iso8601
        }
      end
    end
  end
end
