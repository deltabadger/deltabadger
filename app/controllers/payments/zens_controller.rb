class Payments::ZensController < ApplicationController
  include Payments::Payable

  before_action :authenticate_user!

  def create
    @payment = new_payment_for(
      session[:payment_config]['plan_name'],
      session[:payment_config]['years'],
      session[:payment_config]['type'],
      session[:payment_config]['country']
    )
    @payment.assign_attributes(payment_params)
    @payment.assign_attributes({
                                 total: @payment.price_with_vat,
                                 commission: @payment.referrer_commission_amount,
                                 discounted: @payment.referral_discount_percent.positive?
                               })
    if @payment.save
      result = @payment.get_new_payment_data(locale: I18n.locale)
      if result.success?
        if @payment.update(payment_id: result.data[:payment_id])
          redirect_to result.data[:url]
        else
          flash[:alert] = @payment.errors.messages.values.flatten.to_sentence
          redirect_to upgrade_path
        end
      else
        Raven.capture_exception(Exception.new(result.errors.to_sentence))
        flash[:alert] = t('subscriptions.payment.server_error')
        redirect_to upgrade_path
      end
    else
      flash[:alert] = @payment.errors.messages.values.flatten.to_sentence
      redirect_to upgrade_path
    end
  end

  private

  def payment_params
    params.require(:payments_zen).permit(:first_name, :last_name)
  end
end
