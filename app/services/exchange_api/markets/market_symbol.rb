module ExchangeApi
  module Markets
    MarketSymbol = Struct.new(:base, :quote) do
      def to_s
        "#{base}#{quote}"
      end

      def to_json(_options)
        { base: base, quote: quote }
      end
    end
  end
end
