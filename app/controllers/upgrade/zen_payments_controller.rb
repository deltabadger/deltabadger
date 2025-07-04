class Upgrade::ZenPaymentsController < ApplicationController
  include Upgrades::Payable
  include Upgrades::Showable

  before_action :authenticate_user!
  before_action :update_session, only: %i[create]

  def create
    @payment = new_payment_for(
      plan_name: session[:payment_config]['plan_name'],
      years: session[:payment_config]['years'],
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
      result = @payment.get_new_recurring_payment_data(locale: I18n.locale)
      if result.success?
        if @payment.update(payment_id: result.data[:payment_id])
          redirect_to result.data[:url]
        else
          flash[:alert] = @payment.errors.messages.values.flatten.to_sentence
          set_show_instance_variables
          render 'upgrades/show'
        end
      else
        Raven.capture_exception(Exception.new(result.errors.to_sentence))
        flash[:alert] = t('subscriptions.payment.server_error')
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
    params.require(:payments_zen).permit(:cardholder_name)
  end

  def first_name(cardholder_name)
    cardholder_name.split.first
  end

  def last_name(cardholder_name)
    cardholder_name.split[1..].join(' ').presence
  end

  def update_session
    parsed_params = {
      first_name: first_name(payment_params[:cardholder_name]),
      last_name: last_name(payment_params[:cardholder_name])
    }.compact.stringify_keys
    session[:payment_config].merge!(parsed_params)
  end
end
