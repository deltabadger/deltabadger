class AffiliatesController < ApplicationController
  before_action :authenticate_user!
  before_action :fetch_affiliate!, only: %i[show update_visible_info update_btc_address
                                            confirm_btc_address]
  before_action :ensure_no_affiliate!, only: %i[new create]
  before_action :validate_password!, only: :update_btc_address

  ALL_PERMITTED_PARAMS = Affiliates::Create::INDIVIDUAL_PERMITTED_PARAMS.union(
    Affiliates::Create::EU_COMPANY_PERMITTED_PARAMS
  ).freeze

  def show; end

  def new
    @affiliate = Affiliate.new
    @presenter = Presenters::Affiliates::New.new(@affiliate)
    @reflink = ENV['HOME_PAGE_URL'] + ref_code_path(code: current_user.affiliate.code, locale: nil)
  end

  def create
    result = Affiliates::Create.call(
      user: current_user,
      affiliate_params: affiliate_params.deep_dup
    )

    if result.success?
      Rails.logger.info("Affiliate created for user #{current_user.email}")
      redirect_to affiliate_path, flash: { notice: t('affiliates.program_joined') }
    else
      Rails.logger.error("Affiliate creation failed for user #{current_user.email}")
      @affiliate = current_user.affiliate
      @presenter = Presenters::Affiliates::New.new(@affiliate)
      @reflink = ENV['HOME_PAGE_URL'] + ref_code_path(code: current_user.affiliate.code, locale: nil)
      render :new, status: :unprocessable_entity
    end
  end

  def update_visible_info
    Affiliates::UpdateVisibleInfo.call(affiliate: @affiliate, params: params[:affiliate])

    render :show
  end

  def update_btc_address
    btc_address = params[:affiliate][:btc_address]
    result = Affiliates::UpdateBtcAddress.call(affiliate: @affiliate, new_btc_address: btc_address)

    if result.success?
      flash[:notice] = t('affiliates.btc_address_confirmation_sent')
      render :show
    else
      render :show, status: :unprocessable_entity
    end
  end

  def confirm_btc_address
    result = Affiliates::ConfirmBtcAddress.call(affiliate: @affiliate, token: params[:token])

    if result.success?
      redirect_to affiliate_path, flash: { notice: t('affiliates.btc_address_changed') }
    else
      redirect_to affiliate_path, flash: { alert: result.errors.first }
    end
  end

  private

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

    @affiliate.errors.add(:current_password, 'is incorrect.')

    render :show, status: :unprocessable_entity
  end

  def affiliate_params
    params.require(:affiliate)
  end
end
