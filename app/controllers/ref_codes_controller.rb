class RefCodesController < ApplicationController
  def apply_code
    if !user_signed_in?
      session[:code] = code
      return redirect_to new_user_registration_path
    end

    affiliate = find_affiliate(code)

    if current_user.referrer_id.present?
      redirect_to dashboard_path, flash: { notice: I18n.t('affiliates.discount.already_used') }
    else
      @affiliate = affiliate
    end
  end

  def accept
    if current_user.referrer_id.present?
      redirect_to dashboard_path, flash: { notice: I18n.t('affiliates.discount.already_used') }
      return
    end

    affiliate = find_affiliate(code)

    if affiliate.present?
      current_user.update(referrer_id: affiliate.id)
      redirect_to dashboard_path, flash: { notice: I18n.t('affiliates.discount.accepted') }
    else
      redirect_to dashboard_path, flash: { alert: I18n.t('affiliates.discount.invalid') }
    end
  end

  private

  def find_affiliate(code)
    AffiliatesRepository.new.find_active_by_code(code)
  end

  def code
    params[:code]
  end
end
