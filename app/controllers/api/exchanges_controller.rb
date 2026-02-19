module Api
  class ExchangesController < Api::BaseController
    def index
      api_keys = current_user.api_keys
      exchange_type_pairs = get_exchange_type_pairs(api_keys)

      build_data = lambda do |exchange|
        symbols_query = exchange.symbols
        symbols = symbols_query.success? ? symbols_query.data : []
        all_symbols = exchange.symbols.or([])
        status_of_trading_key = status_of_key(exchange.id, exchange_type_pairs)
        {
          id: exchange.id,
          name: exchange.name,
          maker_fee: exchange.maker_fee || '?',
          taker_fee: exchange.taker_fee || '?',
          symbols:,
          all_symbols:,
          trading_key_status: status_of_trading_key
        }
      end

      render json: { data: Exchange.available.map(&build_data).sort_by { |e| e[:name] } }
    end

    private

    def get_exchange_type_pairs(api_keys)
      api_keys.includes(:exchange).map { |a| { id: a.exchange.id, status: a.status } }
    end

    def status_of_key(id, exchange_type_pairs)
      pair = exchange_type_pairs.find { |e| e[:id] == id }
      return pair if pair.nil?

      pair[:status]
    end
  end
end
