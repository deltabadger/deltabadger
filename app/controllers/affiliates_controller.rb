class AffiliatesController < ApplicationController
  before_action :authenticate_user!
  before_action :fetch_affiliate!, only: [:show, :update_btc_address, :confirm_btc_address]
  before_action :ensure_no_affiliate!, only: [:new, :create]
  before_action :validate_password!, only: :update_btc_address

  def show
    render :show, locals: { affiliate: affiliate, errors: [] }
  end

  def new
    render :new, locals: { affiliate: Affiliate.new, errors: [] }
  end

  def create
    affiliate = Affiliate.new(affiliate_params.merge(default_affiliate_params))
    current_user.affiliate = affiliate
    current_user.save!

    redirect_to affiliate_path
  rescue ActiveRecord::RecordNotSaved
    render :new, locals: { affiliate: affiliate, errors: affiliate.errors.to_a }
  end

  def update_btc_address
    new_btc_address = params[:affiliate][:btc_address]
    result = Affiliates::UpdateBtcAddress.call(affiliate: affiliate, new_btc_address: new_btc_address)

    if result.success?
      flash[:notice] = 'Confirmation email sent'
      render :show, locals: { affiliate: affiliate, errors: []}
    else
      render :show, locals: { affiliate: affiliate, errors: result.errors }
    end
  end

  def confirm_btc_address
    if affiliate.new_btc_address_send_at + 24.hours > Time.now && affiliate.new_btc_address_token == params[:token]
      affiliate.update!(btc_address: affiliate.new_btc_address, new_btc_address_token: nil)
      flash[:notice] = 'Bitcoin address changed'
      render :show, locals: { affiliate: affiliate, errors: [] }
    else
      flash[:alert] = 'Confirmation token is not valid'
      render :show, locals: { affiliate: affiliate, errors: [] }
    end
  end

  private

  attr_reader :affiliate

  def fetch_affiliate!
    @affiliate = current_user.affiliate
    return unless @affiliate.nil?

    redirect_to new_affiliate_path
  end

  def ensure_no_affiliate!
    redirect_to affiliate_path unless current_user.affiliate.nil?
  end

  def validate_password!
    confirmation_password = params[:affiliate][:current_password]
    return if current_user.valid_password?(confirmation_password)

    render :show, locals: { affiliate: affiliate, errors: ['Confirmation password is not valid'] }
  end

  def default_affiliate_params
    { max_profit: 20, discount_percent: 0.2, total_bonus_percent: 0.3 }
  end

  def affiliate_params
    params
      .require(:affiliate)
      .permit(:first_name, :last_name, :birth_date, :eu, :btc_address, :code)
  end
end
