module ExchangeApi::MapErrors
  class Bitstamp < ExchangeApi::MapErrors::Base
    def errors_mapping
      {
        'Check your account balance for details.' => Error.new('Insufficient funds', false)
      }.freeze
    end

    def error_regex_mapping(message)
      insufficient_funds_regex = /.*(Check your account balance for details\.)/.freeze
      return message.match(insufficient_funds_regex).captures[0] if insufficient_funds_regex.match?(message)

      message
    end
  end
end
