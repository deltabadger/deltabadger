# frozen_string_literal: true

module BotApi
  module Bots
    # A signal bot made from the API: one pair, no schedule, and no webhook rule — a rule is a bearer
    # URL that trades, and a bot driven by market_buy / market_sell naming it needs none. Rules can
    # still be added in the app. Created running, because a stopped bot refuses every order.
    class CreateSignal
      include CreateSupport

      REQUIRED = %i[exchange_name base_asset quote_asset].freeze

      def self.call(user:, **opts)
        new(user: user, **opts).call
      end

      def initialize(user:, exchange_name: nil, base_asset: nil, quote_asset: nil, label: nil)
        @user = user
        @exchange_name = exchange_name
        @base_asset = base_asset
        @quote_asset = quote_asset
        @label = label
      end

      def call
        err = missing_required(REQUIRED)
        return err if err

        exchange = find_exchange(@exchange_name)
        return exchange_not_found unless exchange
        return api_key_missing(exchange) unless trading_key(exchange)

        pair = find_pair(exchange, @base_asset, @quote_asset)
        return ticker_not_found(exchange, @base_asset) unless pair

        save_and_start(@user.bots.new(type: 'Bots::Signal', exchange: exchange, label: @label.presence,
                                      settings: pair.transform_keys(&:to_s)))
      end

      private

      def serialize(bot)
        { id: bot.id, label: bot.label, type: bot.type, status: bot.status.to_s, exchange: bot.exchange.name,
          pair: "#{bot.base_asset.symbol}/#{bot.quote_asset.symbol}", started_at: bot.started_at&.iso8601 }
      end
    end
  end
end
