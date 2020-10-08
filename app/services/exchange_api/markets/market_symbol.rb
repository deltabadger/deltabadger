require 'json'

module ExchangeApi
  module Markets
    class MarketSymbol
      attr_reader :base, :quote

      def initialize(base, quote)
        @base = base
        @quote = quote
      end

      def to_json(_options)
        { base: @base, quote: @quote }.to_json
      end
    end
  end
end
