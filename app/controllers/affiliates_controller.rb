class AffiliatesController < ApplicationController
  before_action :authenticate_user!

  def show
    affiliate = current_user.affiliate
    return redirect_to new_affiliate_path if affiliate.nil?
    render :show, locals: { affiliate: affiliate, errors: [] }
  end

  def new
    return redirect_to affiliate_path unless current_user.affiliate.nil?

    render :new, locals: { affiliate: Affiliate.new, errors: [] }
  end

  def create
    return redirect_to affiliate_path unless current_user.affiliate.nil?

    affiliate = Affiliate.new(affiliate_params.merge(default_affiliate_params))
    current_user.affiliate = affiliate
    current_user.save!

    redirect_to affiliate_path
  rescue ActiveRecord::RecordNotSaved
    render :new, locals: { affiliate: affiliate, errors: affiliate.errors.to_a }
  end

  def update_btc_address
    affiliate = current_user.affiliate
    return redirect_to new_affiliate_path if affiliate.nil?

    if current_user.valid_password?(params[:affiliate][:current_password]) &&
       Bitcoin.valid_address?(params[:affiliate][:btc_address])
      flash[:notice] = 'Confirmation email sent'
      render :show, locals: { affiliate: affiliate, errors: []}
    else
      render :show, locals: { affiliate: affiliate, errors: ['Failure']}
    end
  end

  def confirm_btc_address
    byebug
  end

  private

  def default_affiliate_params
    { max_profit: 20, discount_percent: 0.2, total_bonus_percent: 0.3 }
  end

  def affiliate_params
    params
      .require(:affiliate)
      .permit(:first_name, :last_name, :birth_date, :eu, :btc_address, :code)
  end
end
