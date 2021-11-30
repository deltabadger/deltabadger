module Admin
  class GetWireTransfersCommissions < BaseService
    def call
      wire_payments_list = Payment.where(status: 2, wire_transfer: true).where.not(external_statuses: 'Commission granted')
      payments_with_affiliates = wire_payments_list.to_a.filter { |payment| !User.find(payment['user_id'])['referrer_id'].nil? }
      payments_with_affiliates.each do |payment|
        subscription_plan = SubscriptionPlan.find(payment['subscription_plan_id'])
        affiliate = Affiliate.find(User.find(payment['user_id'])['referrer_id'])
        affiliate_commission_percent = affiliate.total_bonus_percent - affiliate.discount_percent
        commission_in_btc = commission_in_btc(payment.currency, affiliate_commission_percent, subscription_plan)
        update_commission(affiliate, commission_in_btc)
        payment.update(external_statuses: 'Commission granted')
      end
    end

    private

    def btc_cost(subscription_plan, currency)
      if currency == 'EUR'
        undiscounted_cost = subscription_plan.cost_eu
        response = Faraday.get('https://api.coinpaprika.com/v1/tickers?quotes=EUR')
        btc_price = JSON.parse(response.body)[0]['quotes']['EUR']['price']
      else
        undiscounted_cost = subscription_plan.cost_other
        response = Faraday.get('https://api.coinpaprika.com/v1/tickers?quotes=USD')
        btc_price = JSON.parse(response.body)[0]['quotes']['USD']['price']
      end
      raise StandardError, "Couldn't fetch BTC price" unless response.status == 200

      undiscounted_cost / btc_price
    end

    def commission_in_btc(currency, commission_percent, subscription_plan)
      btc_cost = btc_cost(subscription_plan, currency)
      (commission_percent * btc_cost).ceil(8)
    end

    def update_commission(affiliate, commission)
      previous_crypto_commission = affiliate.unexported_crypto_commission
      affiliate.update(unexported_crypto_commission: previous_crypto_commission + commission)
    end
  end
end