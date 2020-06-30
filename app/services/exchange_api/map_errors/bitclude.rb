module ExchangeApi
  module MapErrors
    class Bitclude < ExchangeApi::MapErrors::Base
      def errors_mapping
        {}.freeze
      end
    end
  end
end
