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
      # current one can be consumed, taking the steps in between with it. rotp records the last
      # candidate that matched, which is usually the submitted code's own step — a fast device's,
      # say — but when two of the three candidate steps render the same digits, on the order of one
      # submission in a million, the later of them is recorded instead. A correct clock can trigger
      # that, and a slow device can end up two steps past the code it actually sent.
      #
      # None of it can wedge an account, and that is the property worth holding on to: what gets
      # recorded is never more than one step past the step real time was in, and a device inside
      # the tolerated skew always shows a code within one step of real time, so the clock alone
      # clears it — no operator, no reset. How long that takes is deliberately not written down
      # here. Every attempt to put a number on it has been wrong: it turns on the direction of the
      # skew and on which of the candidate steps collided, and a freshly displayed code is not
      # automatically an acceptable one.
      drift = totp.interval
      last_otp_at = totp.verify(code, after: user.last_otp_at, drift_behind: drift, drift_ahead: drift)
      return false if last_otp_at.nil?

      user.update(last_otp_at: Time.at(last_otp_at))
      true
    end
  end
end
