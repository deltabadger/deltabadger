module Users
  class VerifyOtp < BaseService
    def call(user, code)
      totp = ROTP::TOTP.new(user.otp_secret_key)
      last_otp_at = totp.verify(code, after: user.last_otp_at)
      return false if last_otp_at.nil?

      user.update(last_otp_at: Time.at(last_otp_at))
      true
    end
  end
end
