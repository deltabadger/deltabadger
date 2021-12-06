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
        AVAILABLE_WALLET_ADDRESSES = [
          { currency: 'BTC', address: 'fake_btc_address' },
          { currency: 'ETH', address: 'fake_eth_address' }
        ].freeze
        AVAILABLE_CURRENCIES = %w[BTC ETH].freeze

        def withdrawal_minimum(_currency)
          Result::Success.new(0.1)
        end

        def withdrawal_fee(_currency)
          Result::Success.new(0.01)
        end

        def withdrawal_currencies
          Result::Success.new(AVAILABLE_CURRENCIES)
        end

        def available_wallets
          Result::Success.new(AVAILABLE_WALLET_ADDRESSES)
        end

        def available_funds(_bot)
          Result::Success.new(@available_funds)
        end

        def new_account_state
          @available_funds = rand(0.001...2)
        end
      end
    end
  end
end
