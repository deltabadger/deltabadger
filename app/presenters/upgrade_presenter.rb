class UpgradePresenter
  attr_reader :payment

  def initialize(current_user)
    @current_user = current_user
    @payment = crate_new_default_payment

  def allowed_payment_methods
    methods = []
    methods << 'zen' if SettingFlag.show_zen_payment?
    methods << 'btcpay' if SettingFlag.show_bitcoin_payment?
    methods << 'wire_transfer' if SettingFlag.show_wire_payment?
    methods
  end

  def selected_payment_type
    payment.eu? ? 'eu' : 'other'
  end

  def selected_cost_presenter
    all_cost_presenters[@payment.country]
  end

  def selected_plan_name
    case payment.subscription_plan_id
    when pro_plan.id then 'pro'
    when standard_plan.id then 'standard'
    else 'legendary'
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

  def legendary_plan_stats
    @legendary_plan_stats ||= PaymentsManager::LegendaryPlanStatsCalculator.call.data
  end

  def referrer
    return @referrer if defined?(@referrer)

    @referrer = @current_user.eligible_referrer
  end

  def referrer_discount?
    referrer.present?
  end

  def subscription_plan_repository
    @subscription_plan_repository ||= SubscriptionPlansRepository.new
  end

  def standard_plan
    @standard_plan ||= subscription_plan_repository.standard
  end

  def pro_plan
    @pro_plan ||= subscription_plan_repository.pro
  end

  def legendary_plan
    @legendary_plan ||= subscription_plan_repository.legendary
  end

  def legendary_plan_available?
    @number_of_legendary_plans_sold ||= legendary_plan&.subscriptions&.count.to_i
    @number_of_legendary_plans_sold >= 0 && (@number_of_legendary_plans_sold < 1000)
  end

  def available_plans_for_current_user
    @available_plans_for_current_user ||= case current_plan_name
                                          when 'free' then available_plans_for_free_plan
                                          when 'standard' then available_plans_for_standard_plan
                                          when 'pro' then available_plans_for_pro_plan
                                          else []
                                          end
  end

  private

  def crate_new_default_payment
    subscription_plan_id = case current_plan.id
                           when pro_plan.id then legendary_plan.id
                           when standard_plan.id then pro_plan.id
                           else standard_plan.id
                           end

    Payment.new(subscription_plan_id: subscription_plan_id, country: VatRate::NOT_EU)
  end

  def available_plans_for_free_plan
    @available_plans_for_free_plan ||= begin
      available_plans = %w[standard pro]
      available_plans << 'legendary' if legendary_plan_available?
      available_plans
    end
  end

  def available_plans_for_standard_plan
    @available_plans_for_standard_plan ||= begin
      available_plans = %w[pro]
      available_plans << 'standard' if standard_plan_eligibility?
      available_plans << 'legendary' if legendary_plan_available?
      available_plans
    end
  end

  def available_plans_for_pro_plan
    @available_plans_for_pro_plan ||= begin
      available_plans = []
      available_plans << 'legendary' if legendary_plan_available?
      available_plans
    end
  end

  def standard_plan_eligibility?
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
             legendary_plan_discount: legendary_plan_stats[:legendary_plan_discount]
           ).data
         end]
      end.to_h
    end
  end

  def current_plan
    @current_plan ||= @current_user.subscription.subscription_plan
  end
end
