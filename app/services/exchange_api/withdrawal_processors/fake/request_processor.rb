module ExchangeApi
  module WithdrawalProcessors
    module Fake
      class RequestProcessor < BaseRequestProcessor
        include ExchangeApi::Clients::Fake

        SUCCESS = true

        def make_withdrawal(params)
          Result::Success.new(amount: params[:amount], offer_id: rand(2000))
        end
      end
    end
  end
end
