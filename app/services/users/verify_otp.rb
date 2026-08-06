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
      # candidate that matched, which is usually the submitted code's own step; when that step is
      # ahead of real time the sender is a fast device, and with no drift its code would simply
      # have been refused — the widening turned a refusal into a sign-in, which is the point of
      # it. The exception is a collision, on the order of one submission in a million: when the
      # submitted code's step is one of two candidates rendering the same digits, the later of
      # them is recorded. It has to be the submitted step that collides — two other candidates
      # matching each other changes nothing. A correct clock can trigger it, and a slow device
      # can land two steps past the code it actually sent.
      #
      # None of it can wedge an account. Two facts bound the wait, both readable off the call
      # below: the recorded step is never more than one past the step real time was in, because
      # the candidate list stops there; and a device inside the tolerated skew never shows a code
      # more than one step from real time, so its own code is always a candidate. Once real time
      # is two steps past the record, that code sorts later than the record and is accepted — so
      # the third step after the spend, at the worst, whatever the digits collide with. The clock
      # alone clears it: no operator, no reset. That is arithmetic on the two facts rather than a
      # promise; note also that a freshly displayed code is not automatically an acceptable one.
      drift = totp.interval
      last_otp_at = totp.verify(code, after: user.last_otp_at, drift_behind: drift, drift_ahead: drift)
      return false if last_otp_at.nil?

      user.update(last_otp_at: Time.at(last_otp_at))
      true
    end
  end
end
