class RefCodesController < ApplicationController
  def apply_code
    if !user_signed_in?
      session[:code] = code
      return redirect_to new_user_registration_path
    end

    affiliate = Affiliate.active.find_by(code: code)

    if current_user.referrer_id.present?
      redirect_to dashboard_path, flash: { notice: 'You have already used a referral link' }
    else
      @affiliate = affiliate
    end
  end

  def accept
    if current_user.referrer_id.present?
      redirect_to dashboard_path, flash: { notice: 'You have already used a referral link' }
      return
    end

    affiliate = Affiliate.active.find_by(code: code)
    valid = affiliate.present?

    if valid
      current_user.update(referrer_id: affiliate.id)
      redirect_to dashboard_path, flash: { notice: 'You have accepted the referral link' }
    else
      redirect_to dashboard_path, flash: { alert: 'The referral link seems invalid or obsolete' }
    end
  end

  private

  def code
    params[:code]
  end
end
