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
        return Result::Failure.new("Couldn\\'t update the affiliates' data ") unless commission_in_btc.success?

        update_commission(affiliate, commission_in_btc.data)
        payment.update(external_statuses: 'Commission granted')
      end
      Result::Success.new("Wire transfers\\' commissions granted")
    rescue StandardError
      return Result::Failure.new("Couldn\\'t update the affiliates' data ") unless commission_in_btc.success?
    end

    private

    def btc_cost(subscription_plan, currency)
      undiscounted_cost = currency == 'EUR' ? subscription_plan.cost_eu : subscription_plan.cost_other
      btc_price = Admin::GetBitcoinPrice.call(currency)
      return Result::Failure.new("Couldn\\'t fetch Bitcoin price ") unless btc_price.success?

      Result::Success.new(undiscounted_cost / btc_price.data)
    end

    def commission_in_btc(currency, commission_percent, subscription_plan)
      btc_cost = btc_cost(subscription_plan, currency)
      return Result::Failure.new("Couldn\\'t fetch Bitcoin price ") unless btc_cost.success?

      Result::Success.new((commission_percent * btc_cost.data).ceil(8))
    end

    def update_commission(affiliate, commission)
      previous_crypto_commission = affiliate.unexported_crypto_commission
      affiliate.update(unexported_crypto_commission: previous_crypto_commission + commission)
    end
  end
end
