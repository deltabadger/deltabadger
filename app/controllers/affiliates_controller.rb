class AffiliatesController < ApplicationController
  before_action :authenticate_user!
  before_action :fetch_affiliate!, only: %i[show update_visible_info update_btc_address
                                            confirm_btc_address]
  before_action :ensure_no_affiliate!, only: %i[new create]
  before_action :validate_password!, only: :update_btc_address

  ALL_PERMITTED_PARAMS = Affiliates::Create::INDIVIDUAL_PERMITTED_PARAMS.union(
    Affiliates::Create::EU_COMPANY_PERMITTED_PARAMS
  ).freeze

  def show
    render :show, locals: default_show_locals.merge(affiliate: affiliate)
  end

  def new
    render :new, locals: { affiliate: Affiliate.new, errors: [] }
  end

  def create
    result = Affiliates::Create.call(
      user: current_user,
      affiliate_params: affiliate_params.deep_dup
    )

    if result.success?
      redirect_to affiliate_path, flash: { notice: I18n.t('affiliates.program_joined') }
    else
      render :new, locals: {
        affiliate: result.data || Affiliate.new(affiliate_params.permit(ALL_PERMITTED_PARAMS)),
        errors: result.errors
      }
    end
  end

  def update_visible_info
    Affiliates::UpdateVisibleInfo.call(affiliate: affiliate, params: params[:affiliate])

    render :show, locals: default_show_locals.merge(affiliate: affiliate)
  end

  def update_btc_address
    btc_address = params[:affiliate][:btc_address]
    result = Affiliates::UpdateBtcAddress.call(affiliate: affiliate, new_btc_address: btc_address)

    if result.success?
      flash[:notice] = I18n.t('affiliates.btc_address_confirmation_sent')
      render :show, locals: default_show_locals.merge(affiliate: affiliate)
    else
      render :show, locals: default_show_locals.merge(
        affiliate: result.data || affiliate, errors: result.errors
      )
    end
  end

  def confirm_btc_address
    result = Affiliates::ConfirmBtcAddress.call(affiliate: affiliate, token: params[:token])

    if result.success?
      redirect_to affiliate_path, flash: { notice: I18n.t('affiliates.btc_address_changed') }
    else
      redirect_to affiliate_path, flash: { alert: result.errors.first }
    end
  end

  private

  attr_reader :affiliate

  def fetch_affiliate!
    @affiliate = current_user.affiliate
    return unless @affiliate&.btc_address.blank?

    redirect_to new_affiliate_path
  end

  def ensure_no_affiliate!
    redirect_to affiliate_path unless current_user.affiliate&.btc_address.blank?
  end

  def validate_password!
    confirmation_password = params[:affiliate][:current_password]
    return if current_user.valid_password?(confirmation_password)

    affiliate.errors.add(:current_password, 'is incorrect.')

    render :show, locals: default_show_locals.merge(
      affiliate: affiliate, errors: affiliate.errors.full_messages
    )
  end

  def default_show_locals
    {
      errors: [],
      affiliate_active: affiliate.active?
    }
  end

  def affiliate_params
    params
      .require(:affiliate)
  end
end
