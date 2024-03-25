module ExchangeApi::MapErrors
  class Coinbase < ExchangeApi::MapErrors::Base
    def errors_mapping
      {
        'Incorrect scopes. Make sure you have granted all permissions to your API key.' => Error.new('incorrect scopes', false),
        'Permission needed. Make sure you have granted all permissions to your API key.' => Error.new('Permission denied', false),
        'Source Account Not Tradable' => Error.new('Permission denied', false),
        'Insufficient balance in source account' => Error.new('Insufficient funds', false)
      }.freeze
    end
  end
end