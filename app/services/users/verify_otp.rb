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
      # last consumed, so a spent code stays spent. What does change is that a step ahead of the
      # current one can be consumed, which spends the current one along with it. rotp records the
      # last candidate that matched, so that is usually the submitted code's own step — a fast
      # device's — but on the roughly one-in-a-million occasion that two adjacent steps render the
      # same digits, an ordinary current code records the later of the two.
      #
      # The cost is bounded either way: at most one interval passes between spending a step and
      # the next accepted code, and that code is accepted the second it appears. Outside the
      # collision case, spending a step ahead takes an attempt that would have been refused
      # outright before.
      drift = totp.interval
      last_otp_at = totp.verify(code, after: user.last_otp_at, drift_behind: drift, drift_ahead: drift)
      return false if last_otp_at.nil?

      user.update(last_otp_at: Time.at(last_otp_at))
      true
    end
  end
end
