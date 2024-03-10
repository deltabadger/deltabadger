class UpgradePresenter
  attr_reader :payment

  def initialize(current_user)
    @current_user = current_user
    @payment = crate_new_default_payment
  end

  def selected_payment_type
    payment.eu? ? 'eu' : 'other'
  end

  def selected_cost_presenter
    all_cost_presenters[@payment.country]
  end

  def selected_plan_name
    case payment.subscription_plan_id
    when hodler_plan.id then 'hodler'
    when investor_plan.id then 'investor'
    else 'legendary_badger'
    end
  end

  def current_plan_name
    @current_plan_name ||= current_plan.name
  end

  def all_cost_presenters
    @all_cost_presenters ||= all_cost_datas_hash.transform_values do |country|
      country.transform_values do |cost_data|
        CostPresenter.new(cost_data)
      end
    end
  end

  def legendary_badger_stats
    @legendary_badger_stats ||= PaymentsManager::LegendaryBadgerStatsCalculator.call.data
  end

  def referrer_discount?
    referrer.present?
  end

  def referrer
    return @referrer if defined?(@referrer)

    @referrer = @current_user.eligible_referrer
  end

  def subscription_plan_repository
    @subscription_plan_repository ||= SubscriptionPlansRepository.new
  end

  def investor_plan
    @investor_plan ||= subscription_plan_repository.investor
  end

  def hodler_plan
    @hodler_plan ||= subscription_plan_repository.hodler
  end

  def legendary_badger_plan
    @legendary_badger_plan ||= subscription_plan_repository.legendary_badger
  end

  def legendary_badger_plan_available?
    @number_of_legendary_subscriptions ||= legendary_badger_plan&.subscriptions&.count.to_i
    @number_of_legendary_subscriptions.positive? && (@number_of_legendary_subscriptions < 1000)
  end

  def available_plans_for_current_user
    @available_plans_for_current_user ||= case current_plan_name
                                          when 'saver' then available_plans_for_saver
                                          when 'investor' then available_plans_for_investor
                                          when 'hodler' then available_plans_for_hodler
                                          else []
                                          end
  end

  private

  def crate_new_default_payment
    subscription_plan_id = case current_plan.id
                           when hodler_plan.id then legendary_badger_plan.id
                           when investor_plan.id then hodler_plan.id
                           else investor_plan.id
                           end

    Payment.new(subscription_plan_id: subscription_plan_id, country: VatRate::NOT_EU)
  end

  def available_plans_for_saver
    @available_plans_for_saver ||= begin
      available_plans = %w[investor hodler]
      available_plans << 'legendary_badger' if legendary_badger_plan_available?
      available_plans
    end
  end

  def available_plans_for_investor
    @available_plans_for_investor ||= begin
      available_plans = %w[hodler]
      available_plans << 'investor' if investor_plan_eligibility?
      available_plans << 'legendary_badger' if legendary_badger_plan_available?
      available_plans
    end
  end

  def available_plans_for_hodler
    @available_plans_for_hodler ||= begin
      available_plans = []
      available_plans << 'legendary_badger' if legendary_badger_plan_available?
      available_plans
    end
  end

  def investor_plan_eligibility?
    @current_user.subscription.end_time > Time.current + 1.years
  end

  def generate_plans_hash(plans)
    plans_hash = {}
    plans.each do |plan|
      method_name = "#{plan}_plan".to_sym
      plans_hash[plan.to_sym] = send(method_name)
    end
    plans_hash
  end

  def all_cost_datas_hash
    @all_cost_datas_hash ||= begin
      plans = available_plans_for_current_user
      plans_hash = plans.map { |plan| [plan.to_sym, send("#{plan}_plan")] }.to_h

      VatRatesRepository.new.all_in_display_order.map do |country|
        [country.country,
         plans_hash.transform_values do |plan|
           PaymentsManager::CostDataCalculator.call(
             user: @current_user,
             country: country,
             subscription_plan: plan,
             referrer: referrer,
             legendary_badger_discount: legendary_badger_stats[:legendary_badger_discount]
           ).data
         end]
      end.to_h
    end
  end

  def current_plan
    @current_plan ||= @current_user.subscription.subscription_plan
  end
end
