class CalculateFetchingRestartDelay
  BASE_DELAY = 3.seconds.freeze

  def call(restarts)
    raise ArgumentError, 'restarts must be positive' if restarts < 1

    multiplier = 2**(restarts - 1)
    BASE_DELAY * multiplier
  end
end
