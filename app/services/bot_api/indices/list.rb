# frozen_string_literal: true

module BotApi
  module Indices
    # The same set the wizard's picker shows: stock indices only when the Deltabadger data
    # provider is on (CoinGecko-direct cannot serve them).
    class List
      def self.call(exchange_name: nil)
        unless MarketData.configured?
          return Result.failure(:validation_failed, 'market_data_not_configured',
                                'Market data provider is not configured. ' \
                                'Set up CoinGecko or Deltabadger market data in Settings first.')
        end

        scope = MarketDataSettings.deltabadger? ? Index.all : Index.where.not(source: Index::SOURCE_DELTABADGER)
        if exchange_name.present?
          exchange = Exchange.tradeable.find_by('LOWER(name) = ?', exchange_name.to_s.downcase)
          return Result.failure(:not_found, 'exchange_not_found', "Exchange '#{exchange_name}' not found.") unless exchange

          scope = scope.available_on_exchange(exchange)
        end

        names_by_type = Exchange.tradeable.pluck(:type, :name).to_h
        rows = scope.order(:name).map do |index|
          {
            id: index.external_id, name: index.name, source: index.source,
            coins: Array(index.top_coins).size,
            exchanges: (index.available_exchanges || {}).keys.filter_map { |type| names_by_type[type] }.sort
          }
        end
        Result.success({ count: rows.size, indices: rows })
      end
    end
  end
end
