class Upgrade::ZenPaymentsController < ApplicationController
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
      payment_id: payment_params[:payment_id]
    )
    puts "assigning attributes #{payment_params[:finger_print_id]}"
    @payment.assign_attributes({
                                 total: @payment.price_with_vat,
                                 commission: @payment.referrer_commission_amount,
                                 discounted: @payment.referral_discount_percent.positive?,
                                 finger_print_id: payment_params[:finger_print_id]
                               })
    if @payment.save
      # result = @payment.get_new_recurring_payment_data(locale: I18n.locale)
      result = @payment.get_new_payment_data(locale: I18n.locale)
      if result.success? && result.data[:url].present?
        # if @payment.update(url: result.data[:url], recurring: true)
        if @payment.update(url: result.data[:url])
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
    params.require(:payments_zen).permit(:first_name, :last_name, :payment_id, :finger_print_id)
  end

  def update_session
    parsed_params = {
      first_name: payment_params[:first_name],
      last_name: payment_params[:last_name]
    }.compact.stringify_keys
    session[:payment_config].merge!(parsed_params)
  end
end
