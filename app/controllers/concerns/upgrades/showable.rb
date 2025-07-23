module Upgrades::Showable
  extend ActiveSupport::Concern

  private

  def set_show_instance_variables
    @name_pattern = User::Name::PATTERN
    @selected_days = session[:payment_config]['days']
    @reference_payment_options = payment_options_for(available_variant_days.min)
    @reference_duration = SubscriptionPlanVariant.find_by(days: available_variant_days.min).duration
    @payment_options = payment_options_for(@selected_days)
    @available_variant_days = available_variant_days
    @legendary_plan = SubscriptionPlan.legendary
  end

  def payment_options_for(days)
    @payment_options_for ||= {}
    @payment_options_for[days] ||= available_plan_names.map do |plan_name|
      [
        plan_name,
        new_payment_for(
          plan_name: plan_name,
          days: days,
          type: session[:payment_config]['type'],
          country: session[:payment_config]['country'],
          first_name: session[:payment_config]['first_name'],
          last_name: session[:payment_config]['last_name'],
          birth_date: session[:payment_config]['birth_date']
        )
      ]
    end.to_h
  end

  def available_plan_names
    @available_plan_names ||= current_user.available_plan_names
  end

  def available_variant_days
    # @available_variant_days ||= SubscriptionPlanVariant.all_variant_days
    @available_variant_days ||= SubscriptionPlanVariant.all_variant_days - [1460] # exclude 4 years variant
  end
end
