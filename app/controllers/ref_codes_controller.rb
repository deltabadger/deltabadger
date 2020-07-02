class RefCodesController < ApplicationController
  def apply_code
    affiliate = Affiliate.active.find_by(code: code)
    valid, flash = if affiliate
                     session[:referrer_id] = affiliate.id
                     [true, { notice: "Applied affiliate code #{code}"}]
                   else
                     [false, { alert: "Invalid affiliate code #{code}"}]
                   end

    if !user_signed_in?
      redirect_to new_user_registration_path, flash: flash
    elsif current_user.referrer_id
      redirect_to dashboard_path, flash: { alert: "You have already used an affiliate code" }
    elsif valid
      redirect_to ref_code_confirm_path, flash: flash
    else
      redirect_to dashboard_path, flash: flash
    end
  end

  private

  def code
    params[:code]
  end
end
