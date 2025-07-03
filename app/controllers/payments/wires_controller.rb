class Payments::WiresController < ApplicationController
  include Payments::Payable

  before_action :authenticate_user!

  def create
    @payment = new_payment_for(session[:payment_config]['plan_name'], session[:payment_config]['years'],
                               session[:payment_config]['type'], session[:payment_config]['country'])
    @payment.assign_attributes(payment_params)
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
      else
        flash[:alert] = @payment.errors.messages.values.flatten.to_sentence
      end
    else
      flash[:alert] = @payment.errors.messages.values.flatten.to_sentence
    end
    redirect_to upgrade_path
  end

  private

  def payment_params
    params.require(:payments_wire).permit(:first_name, :last_name)
  end
end
