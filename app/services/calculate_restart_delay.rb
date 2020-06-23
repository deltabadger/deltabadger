class CalculateRestartDelay
  BASE_DELAY = 15.minutes.freeze

  def call(restarts)
    raise ArgumentError, 'restarts must be positive' if restarts < 1

    multiplier = 2**(restarts - 1)
    BASE_DELAY * multiplier
  end
end
