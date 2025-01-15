class RefCodesController < ApplicationController
  def apply_code
    if !user_signed_in?
      session[:code] = params[:code]
      return redirect_to new_user_registration_path
    end

    affiliate = Affiliate.find_active_by_code(params[:code])

    if current_user.referrer_id.present?
      redirect_to dashboard_path, flash: { notice: t('affiliates.discount.already_used') }
    elsif current_user.id == affiliate.user_id
      redirect_to referral_program_path
    else
      @affiliate = affiliate
    end
  end

  def accept
    if current_user.referrer_id.present?
      redirect_to dashboard_path, flash: { notice: t('affiliates.discount.already_used') }
      return
    end

    affiliate = Affiliate.find_active_by_code(params[:code])

    if affiliate.present?
      current_user.update(referrer_id: affiliate.id)
      redirect_to dashboard_path, flash: { notice: t('affiliates.discount.accepted') }
    else
      redirect_to dashboard_path, flash: { alert: t('affiliates.discount.invalid') }
    end
  end
end
