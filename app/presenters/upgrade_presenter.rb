class UpgradePresenter
  attr_reader :payment

  def initialize(current_user)
    @current_user = current_user
    @payment = new_default_payment
  end

  def selected_cost_presenter
    all_cost_presenters[@payment.country]
  end

  def all_cost_presenters
    @all_cost_presenters ||= all_cost_datas_hash.transform_values do |country|
      country.transform_values do |cost_data|
        CostPresenter.new(cost_data)
      end
    end
  end

  def referrer
    return @referrer if defined?(@referrer)

    @referrer = @current_user.eligible_referrer
  end

  def referrer_discount?
    referrer.present?
  end

  private

  def all_cost_datas_hash
    @all_cost_datas_hash ||= begin
      plans = available_plans_for_free_plan
      plans_hash = plans.map { |plan| [plan.to_sym, send("#{plan}_plan")] }.to_h

      VatRate.all_in_display_order.map do |vat_rate|
        [vat_rate.country,
         plans_hash.transform_values do |plan|
           PaymentsManager::CostDataCalculator.call(
             payment: Payment.new(subscription_plan: plan, country: vat_rate.country, user: @current_user),
             user: @current_user,
             vat_rate: vat_rate,
             referrer: referrer,
             legendary_plan_discount: SubscriptionPlan.legendary.current_discount
           ).data
         end]
      end.to_h
    end
  end

  def current_plan
    @current_plan ||= @current_user.subscription.subscription_plan
  end
end
