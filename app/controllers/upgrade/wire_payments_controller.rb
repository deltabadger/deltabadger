class Upgrade::WirePaymentsController < ApplicationController
  include Upgrades::Payable
  include Upgrades::Showable

  before_action :authenticate_user!
  before_action :update_session, only: %i[create]

  def create
    @payment = new_payment_for(
      plan_name: session[:payment_config]['plan_name'],
      days: session[:payment_config]['days'],
      type: session[:payment_config]['type'],
      country: session[:payment_config]['country'],
      first_name: session[:payment_config]['first_name'],
      last_name: session[:payment_config]['last_name'],
      birth_date: session[:payment_config]['birth_date']
    )
    @payment.assign_attributes({
                                 total: @payment.price_with_vat,
                                 commission: @payment.referrer_commission_amount,
                                 discounted: @payment.referral_discount_percent.positive?
                               })
    if @payment.save
      if @payment.user.update(
        pending_wire_transfer: @payment.country,
        pending_plan_variant_id: @payment.subscription_plan_variant_id
      )
        @payment.send_wire_transfer_summary
        UpgradeSubscriptionJob.set(wait: 15.minutes).perform_later(@payment)
        redirect_to upgrade_path
      else
        flash[:alert] = @payment.errors.messages.values.flatten.to_sentence
        set_show_instance_variables
        render 'upgrades/show'
      end
    else
      flash[:alert] = @payment.errors.messages.values.flatten.to_sentence
      set_show_instance_variables
      render 'upgrades/show'
    end
  end

  private

  def payment_params
    params.require(:payments_wire).permit(:first_name, :last_name)
  end

  def update_session
    parsed_params = {
      first_name: payment_params[:first_name],
      last_name: payment_params[:last_name]
    }.compact.stringify_keys
    session[:payment_config].merge!(parsed_params)
  end
end
