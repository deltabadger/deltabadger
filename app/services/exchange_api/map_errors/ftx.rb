module ExchangeApi::MapErrors
  class Ftx < ExchangeApi::MapErrors::Base
    def errors_mapping
      {
        'Not enough balances' => Error.new('Insufficient funds', false),
        'Not approved to trade this product' => Error.new('Pair not available', false),
        'Not logged in' => Error.new('Insufficient API key permissions', false),
        'Invalid authenticator code' =>
          Error.new('Disable 2FA for withdrawals in order to use withdrawal bots on FTX', false)
      }.freeze
    end
  end
end
