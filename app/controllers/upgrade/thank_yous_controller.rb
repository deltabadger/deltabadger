class Upgrade::ThankYousController < ApplicationController
  before_action :authenticate_user!

  def show
    @payment = Payments::Zen.new(
      id: 2829,
      status: 'paid',
      total: 1,
      currency: 'eur',
      user: User.first,
      first_name: 'Jan',
      last_name: 'Klosowski',
      subscription_plan_variant: SubscriptionPlanVariant.last,
      country: 'Estonia',
      recurring: true
    )
    render 'upgrades/thank_you'
  end
end
