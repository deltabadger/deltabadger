module ExchangeApi
  module WithdrawalInfo
    module Fake
      class AccountInfoProcessor < BaseAccountInfoProcessor
        include ExchangeApi::Clients::Fake

        def initialize
          super
          new_account_state
        end

        SUCCESS = true
        AVAILABLE_WALLET_ADDRESSES = %w[1234 2345].freeze
        AVAILABLE_CURRENCIES = %w[BTC ETH].freeze

        def withdrawal_currencies
          AVAILABLE_CURRENCIES
        end

        def available_wallets(_currency)
          AVAILABLE_WALLET_ADDRESSES
        end

        def available_funds(_currency)
          @available_funds
        end

        def new_account_state
          @available_funds = rand(0.001...2)
        end
      end
    end
  end
end
