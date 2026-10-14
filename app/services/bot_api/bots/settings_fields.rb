# frozen_string_literal: true

module BotApi
  module Bots
    # Every setting the bot settings page saves, under the page's own field name, with its choices. One
    # table, three readers: the update_bot_settings MCP schema, the REST strong params, and
    # UpdateSettings, which hands the values to the page's own save (Bot::SettingsForm).
    #
    # `kinds` are the bot types whose settings page has the field (basket = Bots::DcaMultiAsset, pair =
    # the retired Bots::DcaSingleAsset), `applies` names them in the refusal. `nullable` fields are the
    # ones the page can blank, which clears them; null clears them here.
    module SettingsFields
      Field = Data.define(:name, :type, :kinds, :applies, :description, :enum, :nullable)

      DCA = %i[basket pair index].freeze
      SCHEDULED = %i[basket pair].freeze
      COMPOSITION = %i[basket].freeze
      ALL_KINDS = %i[basket pair index signal].freeze

      INTERVALS = Automation::Schedulable::INTERVALS.keys.freeze
      TIMEFRAMES = Bot::IndicatorLimitable::INDICATOR_LIMIT_TIMEFRAMES.keys.freeze
      MA_TIMEFRAMES = Bot::MovingAverageLimitable::MOVING_AVERAGE_LIMIT_TIMEFRAMES.keys.freeze
      # The page's trigger "mode" select: restrict = buy/sell only while the condition holds, start =
      # start once it is met, flip = switch between buying and selling when it is met. Price drop has
      # no "while".
      MODES = %w[restrict start flip].freeze
      DROP_MODES = %w[start flip].freeze

      def self.field(name, type, kinds, applies, description, enum: nil, nullable: false)
        Field.new(name: name.to_s, type:, kinds:, applies:, description:, enum:, nullable:)
      end
      private_class_method :field

      # A trading condition: on the scheduled (basket and pair) pages only.
      def self.trigger(name, type, description, enum: nil) = field(name, type, SCHEDULED, 'basket', description, enum:)
      private_class_method :trigger

      def self.triggers
        %w[buy sell].flat_map do |side|
          sell = side == 'sell'
          pfx = sell ? 'sell_' : ''
          verb = sell ? 'selling' : 'buying'
          member = 'Basket bots: the ticker id of the member it watches (get_bot_details lists them).'
          drop_windows = if sell
                           Bot::PriceDropLimitable::PRICE_DROP_LIMIT_SELL_TIME_WINDOW_CONDITIONS.keys
                         else
                           Bot::PriceDropLimitable::PRICE_DROP_LIMIT_BUY_TIME_WINDOW_CONDITIONS.keys
                         end
          [
            trigger("#{pfx}price_limited", 'boolean', "Price condition while #{verb}: on or off."),
            trigger("#{pfx}price_limit_mode", 'string', "Price condition while #{verb}: what it does.", enum: MODES),
            trigger("#{pfx}price_limit_value_condition", 'string',
                    "Price condition while #{verb}: price above, below or between (between needs mode restrict).",
                    enum: Bot::PriceLimitable::PRICE_LIMIT_VALUE_CONDITIONS),
            trigger("#{pfx}price_limit", 'number', "Price condition while #{verb}: the price, in the quote currency."),
            trigger("#{pfx}price_limit_range_lower_bound", 'number', "Price condition while #{verb}: lower price of a between range."),
            trigger("#{pfx}price_limit_range_upper_bound", 'number', "Price condition while #{verb}: upper price of a between range."),
            trigger("#{pfx}price_limit_in_ticker_id", 'number', "Price condition while #{verb}. #{member}"),
            trigger("#{pfx}price_drop_limited", 'boolean', "Price-drop condition while #{verb}: on or off."),
            trigger("#{pfx}price_drop_limit_mode", 'string', "Price-drop condition while #{verb}: what it does.", enum: DROP_MODES),
            trigger("#{pfx}price_drop_limit", 'number',
                    "Price-drop condition while #{verb}: the move, in percent (10 = 10%) #{sell ? 'above the low' : 'below the high'}."),
            trigger("#{pfx}price_drop_limit_time_window_condition", 'string',
                    "Price-drop condition while #{verb}: the #{sell ? 'low' : 'high'} it is measured from.", enum: drop_windows),
            trigger("#{pfx}price_drop_limit_in_ticker_id", 'number', "Price-drop condition while #{verb}. #{member}"),
            trigger("#{pfx}moving_average_limited", 'boolean', "Moving-average condition while #{verb}: on or off."),
            trigger("#{pfx}moving_average_limit_mode", 'string', "Moving-average condition while #{verb}: what it does.", enum: MODES),
            trigger("#{pfx}moving_average_limit_value_condition", 'string',
                    "Moving-average condition while #{verb}: price above or below the average.",
                    enum: Bot::MovingAverageLimitable::MOVING_AVERAGE_LIMIT_VALUE_CONDITIONS),
            trigger("#{pfx}moving_average_limit_in_ma_type", 'string', "Moving-average condition while #{verb}: the average.",
                    enum: Bot::MovingAverageLimitable::MOVING_AVERAGE_LIMIT_MA_TYPES),
            trigger("#{pfx}moving_average_limit_in_timeframe", 'string', "Moving-average condition while #{verb}: candle size.",
                    enum: MA_TIMEFRAMES),
            trigger("#{pfx}moving_average_limit_in_period", 'number', "Moving-average condition while #{verb}: number of candles."),
            trigger("#{pfx}moving_average_limit_in_ticker_id", 'number', "Moving-average condition while #{verb}. #{member}"),
            trigger("#{pfx}indicator_limited", 'boolean', "RSI condition while #{verb}: on or off."),
            trigger("#{pfx}indicator_limit_mode", 'string', "RSI condition while #{verb}: what it does.", enum: MODES),
            trigger("#{pfx}indicator_limit_value_condition", 'string', "RSI condition while #{verb}: indicator above or below the level.",
                    enum: Bot::IndicatorLimitable::INDICATOR_LIMIT_VALUE_CONDITIONS),
            trigger("#{pfx}indicator_limit", 'number', "RSI condition while #{verb}: the level (0-100)."),
            trigger("#{pfx}indicator_limit_in_indicator", 'string', "RSI condition while #{verb}: the indicator.",
                    enum: Bot::IndicatorLimitable::INDICATOR_LIMIT_INDICATORS),
            trigger("#{pfx}indicator_limit_in_timeframe", 'string', "RSI condition while #{verb}: candle size.", enum: TIMEFRAMES),
            trigger("#{pfx}indicator_limit_in_ticker_id", 'number', "RSI condition while #{verb}. #{member}")
          ]
        end
      end
      private_class_method :triggers

      # Catalogue order is the order of the answer's "updated" list; the first five are the original tool's.
      FIELDS = [
        field(:quote_amount, 'number', DCA, 'DCA', 'Amount per order, in the quote currency.'),
        field(:label, 'string', ALL_KINDS, 'any', 'Bot label.'),
        field(:num_coins, 'number', %i[index], 'index', 'Index bots: how many top assets to hold (2-100); the top of the range holds them all.'),
        field(:allocation_flattening, 'number', %i[index], 'index', 'Index bots: 0 = market-cap weights, 1 = equal weights.'),
        field(:allocations, 'string', COMPOSITION, 'basket',
              "Basket bots: weights in percent for any members, e.g. 'BTC:70,ETH:30'. Members not named keep their " \
              'weight (rounded to 0.1, as the page does); weights need not sum to 100 to save, but must to start. ' \
              'Name a member by its key from get_bot_details when two share a symbol, or by asset id.'),
        field(:exchange_id, 'number', ALL_KINDS, 'any', 'Move the bot to another connected exchange (id from list_exchanges).'),
        field(:interval, 'string', DCA, 'DCA', 'Buying interval.', enum: INTERVALS),
        field(:weighting, 'string', COMPOSITION, 'basket', 'Basket bots: manual weights, or weights derived from market cap.',
              enum: ::Bots::DcaMultiAsset::WEIGHTINGS),
        field(:add_asset_id, 'number', COMPOSITION, 'basket', 'Basket bots: add this asset (id) as a member with weight 0.'),
        field(:remove_asset_id, 'number', COMPOSITION, 'basket', 'Basket bots: remove this member (asset id).'),
        field(:normalize_allocations, 'boolean', COMPOSITION, 'basket', 'Basket bots: true scales the weights to sum to 100.'),
        field(:sell_amount, 'number', SCHEDULED, 'basket',
              'While selling a one-asset basket: base amount to sell per sell interval. null clears it.', nullable: true),
        field(:sell_quote_amount, 'number', SCHEDULED, 'basket', 'While selling: quote amount to sell for per sell interval. null clears it.',
              nullable: true),
        field(:sell_interval, 'string', SCHEDULED, 'basket', 'While selling: the selling interval.', enum: INTERVALS),
        field(:smart_intervaled, 'boolean', DCA, 'DCA', 'Smart Intervals (split each order into smaller, more frequent ones): on or off.'),
        field(:smart_interval_quote_amount, 'number', DCA, 'DCA', 'Smart Intervals: size of each smaller buy, in the quote currency.'),
        field(:smart_interval_base_amount, 'number', DCA, 'DCA', 'Smart Intervals while selling a base amount: size of each smaller sell.'),
        field(:limit_ordered, 'boolean', DCA, 'DCA', 'Limit orders instead of market orders: on or off (always on for Hyperliquid).'),
        field(:limit_order_pcnt_distance, 'number', DCA, 'DCA', 'Limit orders: distance from the price, in percent (0.5 = 0.5%).'),
        field(:start_time_enabled, 'boolean', DCA, 'DCA', 'Starting time (delay the first order): on or off.'),
        field(:start_time_mode, 'string', DCA, 'DCA',
              'Starting time: a weekday, a date (start_at), or the next occurrence of an hour (start_time_of_day).',
              enum: Bot::Startable::MODES),
        field(:start_time_of_day, 'string', DCA, 'DCA', "Starting time for weekday or hour modes: 'HH:MM' in the account time zone."),
        field(:start_at, 'string', DCA, 'DCA',
              'Starting time for date mode: a datetime, e.g. 2026-06-15T09:30 in the account time zone or with an offset. null clears it.',
              nullable: true),
        field(:quote_amount_limited, 'boolean', SCHEDULED, 'basket', "Spending cap (don't spend more than) while buying: on or off."),
        field(:quote_amount_limit, 'number', SCHEDULED, 'basket', 'Spending cap, in the quote currency.'),
        field(:base_amount_limited, 'boolean', SCHEDULED, 'basket', "Selling cap (don't sell more than) for a one-asset basket: on or off."),
        field(:base_amount_limit, 'number', SCHEDULED, 'basket', 'Selling cap, in the base asset.'),
        *triggers,
        field(:rebalance_enabled, 'boolean', %i[basket index], 'basket and index', 'Threshold rebalancing: on or off.'),
        field(:rebalance_threshold, 'number', %i[basket index], 'basket and index',
              'Threshold rebalancing: drift that triggers it, in percent (5 = 5%). null clears it (the default applies).', nullable: true)
      ].freeze

      BY_NAME = FIELDS.index_by(&:name).freeze
      NAMES = FIELDS.map(&:name).freeze

      # The kind of bot a settings page belongs to, as `kinds` names it.
      def self.kind(bot)
        if bot.dca_multi_asset? then :basket
        elsif bot.dca_index? then :index
        elsif bot.signal? then :signal
        else :pair
        end
      end

      # The JSON Schema property for MCP.
      def self.schema(field)
        type = field.nullable ? [field.type, 'null'] : field.type
        { type:, description: field.description, enum: field.enum }.compact
      end
    end
  end
end
