class UpgradePresenter
  attr_reader :payment, :current_plan, :investor_plan, :hodler_plan, :legendary_badger_plan, :referrer

  def initialize(payment, current_plan, investor_plan, hodler_plan, legendary_badger_plan, referrer, current_user)
    @payment = payment
    @current_plan = current_plan
    @investor_plan = investor_plan
    @hodler_plan = hodler_plan
    @legendary_badger_plan = legendary_badger_plan
    @referrer = referrer
    @current_user = current_user
  end

  def current_plan_name
    @current_plan_name ||= current_plan.name
  end

  def available_plans
    @available_plans ||= case current_plan.name
                         when 'saver' then available_plans_for_saver
                         when 'investor' then available_plans_for_investor
                         when 'hodler' then available_plans_for_hodler
                         else []
                         end
  end

  def selected_payment_type
    @selected_payment_type ||= payment.eu? ? 'eu' : 'other'
  end

  def selected_plan_name
    if payment.subscription_plan_id == hodler_plan.id
      'hodler'
    elsif payment.subscription_plan_id == investor_plan.id
      'investor'
    else
      'legendary_badger'
    end
  end

  def referrer_discount?
    referrer.present?
  end

  def legendary_badger_plan_available?
    @available ||= legendary_badger_plan&.subscriptions&.count.to_i < 1000
  end

  private

  def available_plans_for_saver
    return %w[investor hodler legendary_badger] if legendary_badger_plan_available?

    %w[investor hodler]
  end

  def available_plans_for_investor
    available_plans = %w[investor hodler legendary_badger]
    available_plans.delete('investor') unless @current_user.subscription.end_time <= Time.current + 1.years
    available_plans.delete('legendary_badger') unless legendary_badger_plan_available?

    available_plans
  end

  def available_plans_for_hodler
    legendary_badger_plan_available? ? %w[legendary_badger] : []
  end
end
