module ExchangeApi::MapErrors
  class Bitfinex < ExchangeApi::MapErrors::Base
    def errors_mapping
      {
        'Invalid order: not enough exchange balance' => Error.new('Insufficient funds', false)
      }.freeze
    end

    def error_regex_mapping(message)
      insufficient_funds_regex = /(Invalid order: not enough exchange balance).*/.freeze
      return message.match(insufficient_funds_regex).captures[0] if insufficient_funds_regex.match?(message)

      message
    end
  end
end
