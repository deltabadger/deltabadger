class BtcpayClient < ApplicationClient
  URL                   = ENV.fetch('BTCPAY_SERVER_URL').freeze
  API_KEY               = ENV.fetch('BTCPAY_API_KEY').freeze
  AUTHORIZATION_HEADER  = ENV.fetch('BTCPAY_AUTHORIZATION_HEADER').freeze

  def self.connection
    @connection ||= Faraday.new(url: URL, **OPTIONS) do |config|
      config.request :json
      config.response :json
      config.response :raise_error
      config.response :logger, Rails.logger, headers: false, bodies: true, log_level: :debug
      config.adapter :net_http_persistent do |http|
        http.idle_timeout = 100
      end
    end
  end

  # @param hash [Hash] A hash of arguments as described in https://medium.com/@lawgate2019/why-should-i-run-btcpayserver-and-become-my-payment-processor-ef7d58e0c3fa
  #   Token is added in this method and not required in the input.
  # @returns
  #   #=> {"facade"=>"pos/invoice",
  #        "data"=>
  #        {"url"=>"https://pay2.deltabadger.com/invoice?id=K2HAjwFvVWpGqypNQtuasL",
  #         "posData"=>nil,
  #         "status"=>"new",
  #         "btcPrice"=>"0.00066713",
  #         "btcDue"=>"0.00066713",
  #         "cryptoInfo"=>
  #          [{"paymentUrls"=>
  #             {"BIP21"=>
  #               "bitcoin:bc1qvn2wgs7mhud494ssle0n4qu9d6zzeua38ky4t4?amount=0.00066713",
  #              "BIP72"=>nil,
  #              "BIP72b"=>nil,
  #              "BIP73"=>nil,
  #              "BOLT11"=>nil},
  #            "cryptoCode"=>"BTC",
  #            "paymentType"=>"BTCLike",
  #            "rate"=>67438.192,
  #            "exRates"=>{"USD"=>0.0},
  #            "paid"=>"0.00000000",
  #            "price"=>"0.00066713",
  #            "due"=>"0.00066713",
  #            "address"=>"bc1qvn2wgs7mhud494ssle0n4qu9d6zzeua38ky4t4",
  #            "url"=>"https://pay2.deltabadger.com/i/BTC/K2HAjwFvVWpGqypNQtuasL",
  #            "totalDue"=>"0.00066713",
  #            "networkFee"=>"0.00000000",
  #            "txCount"=>0,
  #            "cryptoPaid"=>"0.00000000",
  #            "payments"=>[]}],
  #         "price"=>44.99,
  #         "currency"=>"USD",
  #         "exRates"=>{"USD"=>0.0},
  #         "buyerTotalBtcAmount"=>nil,
  #         "itemDesc"=>"Investor Plan Upgrade",
  #         "itemCode"=>nil,
  #         "orderId"=>"379",
  #         "guid"=>"c7ba3fe6-d8ee-463d-85cd-0a6557ae3b4a",
  #         "id"=>"K2HAjwFvVWpGqypNQtuasL",
  #         "invoiceTime"=>1709904448000,
  #         "expirationTime"=>1709946448000,
  #         "currentTime"=>1709904449147,
  #         "lowFeeDetected"=>false,
  #         "btcPaid"=>"0.00000000",
  #         "rate"=>67438.192,
  #         "exceptionStatus"=>false,
  #         "paymentUrls"=>
  #          {"BIP21"=>
  #            "bitcoin:bc1qvn2wgs7mhud494ssle0n4qu9d6zzeua38ky4t4?amount=0.00066713",
  #           "BIP72"=>nil,
  #           "BIP72b"=>nil,
  #           "BIP73"=>nil,
  #           "BOLT11"=>nil},
  #         "refundAddressRequestPending"=>false,
  #         "buyerPaidBtcMinerFee"=>nil,
  #         "bitcoinAddress"=>"bc1qvn2wgs7mhud494ssle0n4qu9d6zzeua38ky4t4",
  #         "token"=>"Mnx7VGpHoH8cbdukiiYRQb",
  #         "flags"=>nil,
  #         "paymentSubtotals"=>{"BTC"=>66713.0},
  #         "paymentTotals"=>{"BTC"=>66713.0},
  #         "amountPaid"=>0,
  #         "minerFees"=>{"BTC"=>{"satoshisPerByte"=>61.0, "totalFee"=>0.0}},
  #         "exchangeRates"=>{"BTC"=>{"USD"=>0.0}},
  #         "supportedTransactionCurrencies"=>{"BTC"=>{"enabled"=>true, "reason"=>nil}},
  #         "addresses"=>{"BTC"=>"bc1qvn2wgs7mhud494ssle0n4qu9d6zzeua38ky4t4"},
  #         "paymentCodes"=>
  #          {"BTC"=>
  #            {"BIP21"=>
  #              "bitcoin:bc1qvn2wgs7mhud494ssle0n4qu9d6zzeua38ky4t4?amount=0.00066713",
  #             "BIP72"=>nil,
  #             "BIP72b"=>nil,
  #             "BIP73"=>nil,
  #             "BOLT11"=>nil}},
  #         "buyer"=>
  #          {"name"=>" ",
  #           "address1"=>nil,
  #           "address2"=>nil,
  #           "locality"=>nil,
  #           "region"=>nil,
  #           "postalCode"=>nil,
  #           "country"=>"Other",
  #           "phone"=>"0001-01-01",
  #           "email"=>"test@test.com"},
  #         "checkoutType"=>nil}}
  def invoice(hash = {})
    with_rescue do
      response = self.class.connection.post do |req|
        req.url '/invoices'
        req.headers = {
          'x-accept-version' => '2.0.0',
          'Accept' => 'application/json',
          'Content-Type' => 'application/json',
          'Authorization' => AUTHORIZATION_HEADER
        }
        req.body = hash.merge(token: API_KEY)
      end
      Result::Success.new(response.body)
    end
  end
end
