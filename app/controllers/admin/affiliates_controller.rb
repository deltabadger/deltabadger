module Admin
  class AffiliatesController < Admin::ApplicationController
    # Overwrite any of the RESTful controller actions to implement custom behavior
    # For example, you may want to send an email after a foo is updated.
    #
    # def update
    #   foo = Foo.find(params[:id])
    #   foo.update(params[:foo])
    #   send_foo_updated_email
    # end
    def index
      repository = AffiliatesRepository.new
      @total_waiting = format_crypto(repository.total_waiting)
      @total_unexported = format_crypto(repository.total_unexported)
      @total_exported = format_crypto(repository.total_exported)
      @total_paid = format_crypto(repository.total_paid)
      super
    end

    # Override this method to specify custom lookup behavior.
    # This will be used to set the resource for the `show`, `edit`, and `update`
    # actions.
    #
    # def find_resource(param)
    #   Foo.find_by!(slug: param)
    # end

    # Override this if you have certain roles that require a subset
    # this will be used to set the records shown on the `index` action.
    #
    # def scoped_resource
    #  if current_user.super_admin?
    #    resource_class
    #  else
    #    resource_class.with_less_stuff
    #  end
    # end

    # See https://administrate-prototype.herokuapp.com/customizing_controller_actions
    # for more information
    def model_name
      :affiliate
    end

    def mark_as_exported
      Affiliates::MarkUnexportedCommissionAsExported.call

      redirect_back(fallback_location: '/')
    end

    def mark_as_paid
      Affiliates::MarkExportedCommissionAsPaid.call

      redirect_back(fallback_location: '/')
    end

    def wallet_csv
      file = Affiliates::GenerateCommissionsWalletCsv.call

      send_data(file, filename: filename('wallet'))
    end

    def accounting_csv
      file = Affiliates::GenerateCommissionsAccountingCsv.call

      send_data(file, filename: filename('accounting'))
    end

    def get_wire_transfers_commissions
      wire_payments_list = Payment.where(status: 2, wire_transfer: true).where.not(external_statuses: 'Commission granted')
      payments_with_affiliates = wire_payments_list.to_a.filter { |payment| !User.find(payment['user_id'])['referrer_id'].nil? }
      payments_with_affiliates.each do |payment|
        subscription_plan = SubscriptionPlan.find(payment['subscription_plan_id'])
        affiliate = Affiliate.find(User.find(payment['user_id'])['referrer_id'])
        affiliate_commission_percent = affiliate.total_bonus_percent - affiliate.discount_percent
        currency = payment.currency
        btc_cost = btc_cost(subscription_plan, currency)
        commission_btc_value = (affiliate_commission_percent * btc_cost).ceil(8)
        previous_crypto_commission = affiliate.unexported_crypto_commission
        affiliate.update(unexported_crypto_commission: previous_crypto_commission + commission_btc_value)
        payment.update(external_statuses: 'Commission granted')
      end
      redirect_back(fallback_location: '/')
    end

    private

    def btc_cost(subscription_plan, currency)
      if currency == 'EUR'
        undiscounted_cost = subscription_plan.cost_eu
        btc_price = JSON.parse(Faraday.get('https://api.coinpaprika.com/v1/tickers?quotes=EUR').body)[0]['quotes']['EUR']['price']
      else
        undiscounted_cost = subscription_plan.cost_other
        btc_price = JSON.parse(Faraday.get('https://api.coinpaprika.com/v1/tickers?quotes=USD').body)[0]['quotes']['USD']['price']
      end
      undiscounted_cost / btc_price
    end

    def format_crypto(amount)
      format('%0.8f', amount)
    end

    def filename(type)
      "deltabadger-commissions-#{type}-#{Time.current.strftime('%F')}.csv"
    end
  end
end
