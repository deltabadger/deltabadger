module Users
  class VerifyOtp < BaseService
    def call(user, code)
      totp = ROTP::TOTP.new(user.otp_secret_key)
      # Tolerate one step of clock skew either side of now, so a device whose clock is off by
      # up to half a minute can still sign in. rotp's drift is in SECONDS, not steps, and it is
      # applied to the timestamp before that is floored into a step — so the value has to be a
      # whole interval. A smaller number widens the window during only part of each step, and a
      # larger one reaches two steps away near a step boundary.
      #
      # Replay protection is unchanged: `after:` keeps only steps strictly later than the one
      # last consumed, so a spent code stays spent. Note that this makes an accepted ahead code
      # spend the current step AND the next one, since it is the code's own step that is
      # recorded. A device fast enough to show that code only shows it during the current step,
      # so the user still waits exactly one code rotation, as they would with no drift at all.
      drift = totp.interval
      last_otp_at = totp.verify(code, after: user.last_otp_at, drift_behind: drift, drift_ahead: drift)
      return false if last_otp_at.nil?

      user.update(last_otp_at: Time.at(last_otp_at))
      true
    end
  end
end
