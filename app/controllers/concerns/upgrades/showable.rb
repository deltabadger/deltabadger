module Upgrades::Showable
  extend ActiveSupport::Concern

  private

  def set_show_instance_variables
    @name_pattern = User::Name::PATTERN
    @selected_years = session[:payment_config]['years']
    @reference_payment_options = payment_options_for(0)
    @payment_options = payment_options_for(@selected_years)
    @available_variant_years = available_variant_years
    @legendary_plan = SubscriptionPlan.legendary
  end

  def payment_options_for(years)
    @payment_options_for ||= {}
    @payment_options_for[years] ||= available_plan_names.map do |plan_name|
      [
        plan_name,
        new_payment_for(
          plan_name: plan_name,
          years: years,
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

  def available_variant_years
    # @available_variant_years ||= SubscriptionPlanVariant.all_variant_years
    @available_variant_years ||= SubscriptionPlanVariant.all_variant_years - [4] # exclude 4 years variant
  end
end
